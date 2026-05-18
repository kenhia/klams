//! klams-viewport: Tauri 2 desktop app for inspecting the klams memory
//! service. The Tauri command surface lives in `commands::`.
//!
//! CLI flags:
//!   --debug   Open `WebView` devtools on launch and write per-poll
//!             diagnostic lines to `%TEMP%/klams-viewport.log`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;

use commands::{health, memory, AppState};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

pub static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn debug_enabled() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

fn log_path() -> std::path::PathBuf {
    std::env::temp_dir().join("klams-viewport.log")
}

pub fn log_line(msg: &str) {
    if !debug_enabled() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        let _ = writeln!(f, "{msg}");
    }
}

fn install_panic_hook() {
    let path = log_path();
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "PANIC: {info}"));
    }));
}

fn main() {
    let debug = std::env::args().any(|a| a == "--debug");
    DEBUG.store(debug, Ordering::Relaxed);
    install_panic_hook();
    if debug {
        let _ = std::fs::write(log_path(), b""); // truncate
        log_line(&format!(
            "start pid={} exe={:?} debug=true",
            std::process::id(),
            std::env::current_exe().ok()
        ));
    }
    tauri::Builder::default()
        .manage(AppState::live())
        .invoke_handler(tauri::generate_handler![
            memory::list_facts,
            memory::list_events,
            memory::search_unified,
            memory::search_knowledge,
            memory::get_fact,
            memory::get_event,
            memory::get_knowledge_item,
            memory::get_config,
            memory::set_config,
            memory::list_dissents,
            memory::get_dissent,
            memory::promote_dissent,
            memory::discard_dissent,
            memory::upsert_fact,
            memory::edit_fact,
            memory::delete_fact,
            health::get_health,
        ])
        .setup(|app| {
            health::spawn_poll(app.handle().clone());
            if debug_enabled() {
                use tauri::Manager;
                if let Some(w) = app.get_webview_window("main") {
                    log_line(&format!(
                        "setup: main window url()={:?}",
                        w.url().map(|u| u.to_string())
                    ));
                    w.open_devtools();
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running klams-viewport");
}
