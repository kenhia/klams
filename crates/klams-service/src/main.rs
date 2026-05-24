//! klams-service binary entrypoint.
//!
//! Loads runtime config, initializes logging, connects to Postgres /
//! Qdrant / TEI, spawns the worker pool, and serves the HTTP API.
//! Module bodies live in the sibling library crate (`src/lib.rs`).

use klams_service::backup::{self as service_backup, MaintenanceState, OrchestratorDeps};
use klams_service::{config, logging};

use anyhow::{Context, Result};
use klams_api::{build_router, with_metrics, ApiState};
use klams_core::{spawn_workers, DecayTask, LastUsedBumper, MemoryQueue, ValidatorRegistry};
use klams_store::{CompositeStore, PostgresStore, QdrantStore, TeiEmbedder};
use std::sync::Arc;
use tokio::signal;
use tracing::info;

use klams_service::config::LogResolvedDecay;

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<()> {
    let config_path =
        std::env::var("KLAMS_CONFIG").unwrap_or_else(|_| "/ai/klams/config/klams.toml".to_string());

    // Sprint 006 (T013) — `--validate-backup-config` early-out.
    // Loads the config, prints a single line, exits 0/2 without
    // starting the service or contacting any backend.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--validate-backup-config") {
        validate_backup_config_cli(&config_path);
    }

    // Sprint 006 (T028) — `--run-backup-now` ad-hoc trigger. Loads
    // the config and runs one backup synchronously without starting
    // the HTTP server or the scheduler. Used by `just backup-once`.
    let run_now = args.iter().any(|a| a == "--run-backup-now");

    // Sprint 006 (T034) — `--restore-from <date> [--force]` driver.
    // Same one-shot pattern as --run-backup-now.
    let restore_from = arg_value(&args, "--restore-from");
    let restore_force = args.iter().any(|a| a == "--force");

    let cfg = config::Config::from_path(&config_path)
        .with_context(|| format!("loading config from {config_path}"))?;

    logging::init(&cfg.logging.format, &cfg.logging.level);
    info!(config = %config_path, "klams-service starting");

    if let Err(err) = cfg.backup.validate() {
        tracing::error!(error = %err, "invalid [backup] config; refusing to start");
        std::process::exit(2);
    }
    for warning in cfg.backup.warnings() {
        tracing::warn!("{warning}");
    }
    service_backup::metrics::describe();
    service_backup::metrics::set_maintenance_active(false);
    let maintenance_state = MaintenanceState::new();

    if run_now {
        run_backup_now_cli(&cfg, &maintenance_state).await;
    }
    if let Some(date) = restore_from {
        run_restore_from_cli(&cfg, &maintenance_state, &date, restore_force).await;
    }

    let postgres = PostgresStore::connect(&cfg.postgres.url, cfg.postgres.max_connections)
        .await
        .context("connecting to postgres")?;
    let qdrant = QdrantStore::connect(
        &cfg.qdrant.grpc_url,
        &cfg.qdrant.collection,
        u64::from(cfg.embeddings.vector_dim),
    )
    .await
    .context("connecting to qdrant")?;
    let embedder = TeiEmbedder::new(
        cfg.embeddings.url.clone(),
        cfg.embeddings.vector_dim as usize,
    )
    .context("building TEI client")?;
    let (bumper, bumps_rx) = LastUsedBumper::channel();
    let store =
        Arc::new(CompositeStore::new(postgres, qdrant, embedder).with_bump_sender(bumper.sender()));

    cfg.decay.log_resolved();
    if let Err(err) = cfg.decay.validate() {
        tracing::error!(error = %err, "invalid [decay] config; refusing to start");
        std::process::exit(2);
    }
    klams_core::metrics::incr_decay_config_reloads();
    info!(
        task_fact_lambda = cfg.decay.lambda_for(klams_types::FactType::TaskFact),
        user_fact_lambda = cfg.decay.lambda_for(klams_types::FactType::UserFact),
        env_fact_lambda = cfg.decay.lambda_for(klams_types::FactType::EnvFact),
        interval = cfg.decay.task_interval_seconds,
        batch = cfg.decay.batch_size,
        "decay config loaded"
    );
    let decay_task = DecayTask::new(cfg.decay.clone(), Arc::clone(&store)).with_bumps_rx(bumps_rx);
    let _decay_handle = tokio::spawn(decay_task.run());

    let (queue, rx) = MemoryQueue::new(cfg.queue.capacity);
    let _workers = spawn_workers(cfg.queue.workers, rx, Arc::clone(&store));

    let token_mode = klams_core::tokens::TokenMode::from_config_str(&cfg.tokens.mode);
    let token_counter = klams_core::tokens::TokenCounter::new(token_mode);
    info!(
        encoder = token_counter.encoder_id().as_str(),
        configured_mode = %cfg.tokens.mode,
        "context token counter ready"
    );
    let context_builder = Arc::new(
        klams_core::context::ContextBuilder::new(token_counter, cfg.retrieval.per_source_top_k)
            .with_summary_store(Arc::clone(&store) as Arc<dyn klams_store::SummaryStore>),
    );

    // Sprint 005 (T040) — spawn the summarization task.
    {
        use klams_core::summarize::{
            StoreEventSource, SummarizationConfig as SCfg, SummarizationTask,
        };
        let scfg = SCfg {
            enabled: cfg.summarization.enabled,
            event_cluster_min: cfg.summarization.event_cluster_min,
            llm_fallback: cfg.summarization.llm_fallback,
            task_interval: std::time::Duration::from_secs(cfg.summarization.task_interval_seconds),
            ollama_url: cfg.summarization.ollama_url.clone(),
            ollama_model: cfg.summarization.ollama_model.clone(),
        };
        let task = SummarizationTask::new(
            scfg,
            Arc::new(StoreEventSource::new(Arc::clone(&store))),
            Arc::clone(&store) as Arc<dyn klams_store::SummaryStore>,
        );
        let _ = task.spawn();
    }

    let state = ApiState {
        store: Arc::clone(&store),
        queue,
        queue_capacity: cfg.queue.capacity,
        workers: cfg.queue.workers,
        started_at: std::time::Instant::now(),
        validators: Arc::new(ValidatorRegistry::with_defaults()),
        context_builder,
        maintenance: maintenance_state.clone(),
    };
    let router = with_metrics(build_router(state, cfg.auth.bearer_token.clone()));
    klams_core::metrics::describe();

    // Sprint 006 (T024/T027) — stale-lockfile recovery + scheduler.
    if cfg.backup.enabled {
        if let Some(dir) = cfg.backup.backup_dir.clone() {
            match service_backup::lifecycle::recover_stale_lock(&dir).await {
                Ok(Some(recovered)) => {
                    tracing::warn!(
                        pid = recovered.pid,
                        run_id = %recovered.run_id,
                        "recovered stale backup lockfile (service_restarted_mid_backup)"
                    );
                    service_backup::metrics::incr_runs_total(false);
                }
                Ok(None) => {}
                Err(e) => tracing::error!(error = %e, "stale lockfile recovery failed"),
            }
            let deps = orchestrator_deps_from_config(&cfg, dir.clone(), maintenance_state.clone());
            let window = cfg.backup.window_start_utc;
            tokio::spawn(async move {
                service_backup::scheduler::run(deps, window).await;
            });
            info!(
                window = %window,
                backup_dir = %dir.display(),
                "backup scheduler started"
            );
        }
    } else {
        info!("backup feature disabled ([backup].enabled=false)");
    }

    let addr = format!("{}:{}", cfg.server.listen_addr, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!(%addr, "listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;

    info!("klams-service stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

/// Sprint 006 (T013) — `klams-service --validate-backup-config`.
///
/// Loads `klams.toml`, runs [`BackupConfig::validate`], prints either
/// `OK: ...` or the error, and exits with `0` on success or `2` on
/// any error. No backends are contacted, no logging is initialized.
/// Never returns.
fn validate_backup_config_cli(config_path: &str) -> ! {
    let cfg = match config::Config::from_path(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config load error ({config_path}): {e}");
            std::process::exit(2);
        }
    };
    match cfg.backup.validate() {
        Ok(()) => {
            if cfg.backup.enabled {
                println!(
                    "OK: [backup] enabled, dir={}, window_start_utc={}, daily={}, weekly={}",
                    cfg.backup
                        .backup_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    cfg.backup.window_start_utc,
                    cfg.backup.daily_count,
                    cfg.backup.weekly_count,
                );
            } else {
                println!("OK: [backup] disabled (enabled=false)");
            }
            for warning in cfg.backup.warnings() {
                eprintln!("warning: {warning}");
            }
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("[backup] config invalid: {e}");
            std::process::exit(2);
        }
    }
}

/// Derive a Qdrant REST URL from the configured gRPC URL. Qdrant
/// places its REST API on the gRPC port minus 1 by default
/// (6334 gRPC ↔ 6333 REST). Returns the original URL if the port
/// can't be parsed.
fn derive_qdrant_rest_url(grpc_url: &str) -> String {
    match url::Url::parse(grpc_url) {
        Ok(mut u) => {
            if let Some(p) = u.port() {
                let _ = u.set_port(Some(p.saturating_sub(1)));
            }
            u.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => grpc_url.to_string(),
    }
}

fn orchestrator_deps_from_config(
    cfg: &config::Config,
    backup_dir: std::path::PathBuf,
    state: MaintenanceState,
) -> OrchestratorDeps {
    OrchestratorDeps {
        backup_dir,
        pg_url: cfg.postgres.url.clone(),
        pg_bin_dir: cfg.backup.pg_bin_dir.clone(),
        qdrant_rest_url: derive_qdrant_rest_url(&cfg.qdrant.grpc_url),
        qdrant_collection: cfg.qdrant.collection.clone(),
        daily_count: cfg.backup.daily_count,
        weekly_count: cfg.backup.weekly_count,
        same_day_strategy: cfg.backup.same_day_strategy,
        drop_remote_qdrant_snapshot: true,
        state,
        status_hook: cfg.backup.status_hook.clone(),
        status_hook_timeout: cfg.backup.status_hook_timeout,
    }
}

/// Sprint 006 (T028) — `klams-service --run-backup-now`.
///
/// Runs one backup synchronously and exits 0 on success / 2 on
/// failure. Used by `just backup-once`. Never returns.
async fn run_backup_now_cli(cfg: &config::Config, state: &MaintenanceState) -> ! {
    let Some(dir) = cfg.backup.backup_dir.clone() else {
        eprintln!("[backup] cannot --run-backup-now: backup_dir is unset");
        std::process::exit(2);
    };
    if !cfg.backup.enabled {
        eprintln!("[backup] note: enabled=false, but running ad-hoc anyway (--run-backup-now)");
    }
    service_backup::metrics::describe();
    let deps = orchestrator_deps_from_config(cfg, dir, state.clone());
    match service_backup::run_once(&deps).await {
        Ok(run) => {
            let ok = run.ok.unwrap_or(false);
            println!(
                "{} run_id={} duration_ms={} artifacts={}",
                if ok { "OK" } else { "FAIL" },
                run.run_id,
                run.duration_ms().unwrap_or(0),
                run.artifacts.len()
            );
            std::process::exit(if ok { 0 } else { 2 });
        }
        Err(e) => {
            eprintln!("[backup] run_once error: {e}");
            std::process::exit(2);
        }
    }
}

/// Find the value following a `--flag` in `args`, supporting both
/// `--flag value` and `--flag=value` forms. Returns `None` if not
/// present or if the next token also starts with `--`.
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    for (i, a) in args.iter().enumerate() {
        if let Some(v) = a.strip_prefix(&prefix) {
            return Some(v.to_string());
        }
        if a == flag {
            if let Some(next) = args.get(i + 1) {
                if !next.starts_with("--") {
                    return Some(next.clone());
                }
            }
        }
    }
    None
}

/// Sprint 006 (T034) — `klams-service --restore-from <date> [--force]`.
///
/// Runs one restore synchronously and exits 0/2. Used by
/// `just restore-from`. Never returns.
async fn run_restore_from_cli(
    cfg: &config::Config,
    state: &MaintenanceState,
    date: &str,
    force: bool,
) -> ! {
    let Some(dir) = cfg.backup.backup_dir.clone() else {
        eprintln!("[backup] cannot --restore-from: backup_dir is unset");
        std::process::exit(2);
    };
    service_backup::metrics::describe();
    let deps = orchestrator_deps_from_config(cfg, dir, state.clone());
    let started = std::time::Instant::now();
    let result = service_backup::restore::run_from(&deps, date, force, |evt| match evt {
        service_backup::restore::RestoreProgress::Resolved { pg_path, q_path } => {
            println!(
                "resolved pg={} qdrant={}",
                pg_path.display(),
                q_path.display()
            );
        }
        service_backup::restore::RestoreProgress::PgRestoreStarted => {
            println!("postgres restore: started");
        }
        service_backup::restore::RestoreProgress::PgRestoreDone => {
            println!("postgres restore: done");
        }
        service_backup::restore::RestoreProgress::QdrantRestoreStarted => {
            println!("qdrant restore: started");
        }
        service_backup::restore::RestoreProgress::QdrantRestoreDone => {
            println!("qdrant restore: done");
        }
    })
    .await;
    let elapsed_ms = started.elapsed().as_millis();
    match result {
        Ok(()) => {
            println!("OK restore date={date} elapsed_ms={elapsed_ms}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("FAIL restore date={date} elapsed_ms={elapsed_ms} error={e}");
            std::process::exit(2);
        }
    }
}
