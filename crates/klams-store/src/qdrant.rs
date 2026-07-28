//! Qdrant adapter (knowledge items).

use crate::{StoreError, StoreResult};
use klams_types::{IndexKnowledge, KnowledgeItem, Source};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, points_selector::PointsSelectorOneOf, value::Kind as ValueKind,
    Condition, CountPointsBuilder, CreateCollectionBuilder, DeletePointsBuilder, Distance,
    FieldType, Filter, ListValue, PointId, PointStruct, PointsIdsList, QueryPointsBuilder,
    ScrollPointsBuilder, SetPayloadPointsBuilder, UpsertPointsBuilder, Value, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use std::collections::HashMap;
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Convert a `chrono::DateTime<Utc>` to a `time::OffsetDateTime` so it
/// can be compared against the RFC3339 `created_at` payload Qdrant
/// stores. Used by sprint 008 cross-author page queries.
fn chrono_to_offset(ts: chrono::DateTime<chrono::Utc>) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ts.timestamp_nanos_opt().unwrap_or(0)))
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
}
use uuid::Uuid;

const KEYWORD_INDEX_FIELDS: &[&str] = &[
    "content_hash",
    "source",
    "tags",
    "repo",
    "machine",
    "file",
    // Sprint 028 (#642): one point per content hash. `machines`/`files`
    // are list payloads over every copy of the content (a keyword index
    // over a list matches any element); the singular `machine`/`file`
    // above remain the canonical copy.
    "machines",
    "files",
    // Sprint 009 (kwi #32 followup, T048): enable fast filtered
    // count/scroll by author so `/v1/authors` can include
    // `counts.knowledge` without a full payload scan.
    "author_id",
];

/// Filter for `list_knowledge_by_author`.
#[derive(Debug, Clone, Copy)]
pub enum AuthorMemoryStateFilter {
    Live,
    Deleted,
    All,
}

#[derive(Clone)]
pub struct QdrantStore {
    client: Arc<Qdrant>,
    collection: String,
    vector_dim: u64,
    /// Serializes copy bookkeeping (sprint 028 #642). Attach and
    /// per-host delete are read-modify-write on the `copies` payload;
    /// Qdrant has no transactions, so concurrent bookkeeping from the
    /// handler, the workers, and two scanners could drop an entry — and
    /// a dropped entry means a later delete-for-the-other-host removes
    /// the point while a host still has the file. These ops are rare
    /// (dedupe hits and deletes only), so one process-wide mutex costs
    /// nothing.
    bookkeeping: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for QdrantStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantStore")
            .field("client", &"<qdrant_client>")
            .field("collection", &self.collection)
            .field("vector_dim", &self.vector_dim)
            .finish_non_exhaustive()
    }
}

impl QdrantStore {
    /// Connect, ensure the configured collection (production:
    /// `knowledge_items_v2` since sprint 028) and payload indexes
    /// exist. Note this *creates on absence* — a wrong collection name
    /// manufactures an empty collection instead of failing.
    pub async fn connect(grpc_url: &str, collection: &str, vector_dim: u64) -> StoreResult<Self> {
        let client = Qdrant::from_url(grpc_url)
            .build()
            .map_err(|e| StoreError::Backend(format!("qdrant client: {e}")))?;

        let exists = client
            .collection_exists(collection)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant exists: {e}")))?;
        if !exists {
            // `collection_exists` + `create_collection` is check-then-act,
            // so two processes connecting at the same moment both see
            // "absent" and both create — the loser gets `Collection ...
            // already exists!`. Losing that race is not a failure: the
            // collection is there, which is all the caller wanted.
            //
            // Sprint 031: latent since 007 and only ever fired in tests
            // (the shared test collection always pre-existed, until
            // #687's sweep started dropping it). It is equally reachable
            // in production — klams-service, klams-scanner and
            // klams-monitor all call `connect`, and a fresh Qdrant plus
            // a simultaneous start is the same race.
            if let Err(e) = client
                .create_collection(
                    CreateCollectionBuilder::new(collection)
                        .vectors_config(
                            VectorParamsBuilder::new(vector_dim, Distance::Cosine).on_disk(true),
                        )
                        .on_disk_payload(true),
                )
                .await
            {
                let msg = e.to_string();
                if !msg.contains("already exists") {
                    return Err(StoreError::Backend(format!("qdrant create: {msg}")));
                }
            }
        }
        for field in KEYWORD_INDEX_FIELDS {
            let _ = client
                .create_field_index(
                    qdrant_client::qdrant::CreateFieldIndexCollectionBuilder::new(
                        collection.to_string(),
                        (*field).to_string(),
                        FieldType::Keyword,
                    ),
                )
                .await;
        }

        // #54: a datetime index on the RFC3339 `created_at` payload lets the
        // memories feed scroll newest-first via `order_by` (see
        // `list_memories_knowledge_page`). Idempotent — Qdrant builds it over
        // existing points; a re-run or an already-present index is a no-op, so
        // the error is ignored exactly like the keyword indexes above.
        let _ = client
            .create_field_index(
                qdrant_client::qdrant::CreateFieldIndexCollectionBuilder::new(
                    collection.to_string(),
                    "created_at".to_string(),
                    FieldType::Datetime,
                ),
            )
            .await;

        Ok(Self {
            client: Arc::new(client),
            collection: collection.to_string(),
            vector_dim,
            bookkeeping: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Read access to the underlying Qdrant gRPC client. Required by
    /// the sprint-007 author backfill module (`backfill_qdrant_authors`)
    /// to drive scroll + `set_payload` pages without re-exposing the
    /// full upsert/search surface.
    pub fn client(&self) -> &Qdrant {
        &self.client
    }

    /// Upsert a knowledge point with a caller-supplied embedding and
    /// content hash. Returns the persisted `KnowledgeItem`.
    pub async fn index_knowledge(
        &self,
        req: IndexKnowledge,
        embedding: Vec<f32>,
    ) -> StoreResult<KnowledgeItem> {
        if embedding.len() as u64 != self.vector_dim {
            return Err(StoreError::Other(format!(
                "embedding dim {} != collection dim {}",
                embedding.len(),
                self.vector_dim
            )));
        }
        let now = OffsetDateTime::now_utc();
        let item = KnowledgeItem {
            id: req.id,
            text: req.text.clone(),
            content_hash: req.content_hash.clone(),
            source: req.source,
            tags: req.tags.clone(),
            repo: req.repo.clone(),
            file: req.file.clone(),
            machine: req.machine.clone(),
            machines: req.machine.iter().cloned().collect(),
            heading_path: req.heading_path.clone(),
            language: req.language.clone(),
            chunk_index: req.chunk_index,
            volatility: req.volatility.clone(),
            supersedes: req.supersedes,
            superseded_by: None,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: now,
            updated_at: now,
        };
        let mut payload = item_to_payload(&item);
        payload.insert("author_id".into(), Value::from(req.author_id.to_string()));
        // Sprint 028 (#642): scanner content starts its copy bookkeeping
        // with itself as the only — and canonical — copy. Agent memories
        // (no machine and no file) carry none.
        if req.machine.is_some() || req.file.is_some() {
            let copies = vec![CopyEntry {
                machine: req.machine.clone(),
                file: req.file.clone(),
                repo: req.repo.clone(),
            }];
            let (copy_payload, _cleared) = copies_payload(&copies);
            payload.extend(copy_payload);
        }
        // Sprint 022 (#322) — chunk structure metadata in the payload so
        // neighbour expansion, section-heading retrieval, and the graph
        // layer have structure to query. Written when present; absent
        // fields simply aren't stored (back-compatible with old points).
        if let Some(ci) = req.chunk_index {
            payload.insert("chunk_index".into(), Value::from(i64::from(ci)));
        }
        if let Some(lang) = &req.language {
            payload.insert("language".into(), Value::from(lang.clone()));
        }
        if let Some(hp) = &req.heading_path {
            payload.insert("heading_path".into(), Value::from(hp.clone()));
        }
        if !req.symbols.is_empty() {
            payload.insert(
                "symbols".into(),
                Value {
                    kind: Some(ValueKind::ListValue(ListValue {
                        values: req.symbols.iter().map(|s| Value::from(s.clone())).collect(),
                    })),
                },
            );
        }
        let point = PointStruct::new(
            PointId {
                point_id_options: Some(PointIdOptions::Uuid(item.id.to_string())),
            },
            embedding,
            payload,
        );
        self.client
            .upsert_points(
                UpsertPointsBuilder::new(self.collection.clone(), vec![point]).wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant upsert: {e}")))?;
        Ok(item)
    }

    pub async fn search_knowledge(
        &self,
        query_vector: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        // Exclude soft-deleted points (sprint 007 R-003) — points whose
        // payload carries a non-null `deleted_at`.
        //
        // Sprint 032 (#335) dropped a companion
        // `must_not: kind = "digest"` clause. It excluded knowledge
        // digests (sprint 005 T038) from raw search, but T038 was never
        // wired: nothing ever wrote a digest point, both live
        // collections held zero, and the writer has now been deleted.
        let filter = Filter {
            must: vec![Condition::is_empty("deleted_at")],
            ..Default::default()
        };
        // Sprint 032 (#334): the universal `query_points` API replaces the
        // legacy `search_points`. Same semantics for a plain dense ANN
        // query, and it is the entry point Qdrant's server-side hybrid /
        // prefetch features hang off, so the migration is also the door
        // to #333 if lexical search is ever done in-engine.
        let resp = self
            .client
            .query(
                QueryPointsBuilder::new(self.collection.clone())
                    .query(query_vector)
                    .limit(u64::from(top_k))
                    .with_payload(true)
                    .filter(filter),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant search: {e}")))?;
        let mut out = Vec::with_capacity(resp.result.len());
        for sp in resp.result {
            let payload = sp.payload;
            if let Some(item) = payload_to_item(&payload) {
                out.push((item, sp.score));
            }
        }
        Ok(out)
    }

    /// ANN search restricted to the curated stratum: agent-authored
    /// knowledge (`source = "AgentProposal"`), live points only
    /// (sprint 029, #644).
    ///
    /// The stratum is tiny (~100 points in a ~180k corpus), so this
    /// always surfaces the best curated matches for a query regardless
    /// of their global ANN rank — the fix for #628's Query A class,
    /// where a badly-phrased query misses the curated target in any
    /// global top-k. Reuses the caller's query vector; costs
    /// microseconds. Also serves `memory_add`'s similar-on-write check
    /// (#638), which is the same question at write time.
    pub async fn search_knowledge_curated(
        &self,
        query_vector: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        // `is_empty("machine")` is load-bearing: the corpus holds
        // file-derived AgentProposal points (scanned session
        // transcripts, machine set) that are NOT curated writes —
        // without this they flood the stratum (measured 2026-07-26).
        let filter = Filter {
            must: vec![
                Condition::is_empty("deleted_at"),
                Condition::is_empty("machine"),
                Condition::matches("source", Source::AgentProposal.as_str().to_string()),
            ],
            ..Default::default()
        };
        let resp = self
            .client
            .query(
                QueryPointsBuilder::new(self.collection.clone())
                    .query(query_vector)
                    .limit(u64::from(top_k))
                    .with_payload(true)
                    .filter(filter),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant curated search: {e}")))?;
        let mut out = Vec::with_capacity(resp.result.len());
        for sp in resp.result {
            if let Some(item) = payload_to_item(&sp.payload) {
                out.push((item, sp.score));
            }
        }
        Ok(out)
    }

    pub async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>> {
        // Content-only (sprint 028 #642): identical content is one point
        // wherever it appears; the sprint 022/023 file/host scoping moved
        // into the `copies` payload bookkeeping. Live points only — a
        // scanner chunk deduping onto a soft-deleted memory would make
        // live content unsearchable.
        let filter = Filter {
            must: vec![
                Condition::matches("content_hash", hash.to_string()),
                Condition::is_empty("deleted_at"),
            ],
            ..Default::default()
        };
        let resp = self
            .client
            .scroll(
                ScrollPointsBuilder::new(self.collection.clone())
                    .filter(filter)
                    .limit(1)
                    .with_payload(false)
                    .with_vectors(false),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant scroll: {e}")))?;
        Ok(resp
            .result
            .into_iter()
            .next()
            .and_then(|p| match p.id?.point_id_options? {
                PointIdOptions::Uuid(s) => Uuid::parse_str(&s).ok(),
                PointIdOptions::Num(_) => None,
            }))
    }

    /// Record that (`machine`, `file`) also holds the content of point
    /// `id` (sprint 028 #642): appends a copy entry and refreshes the
    /// derived `machines`/`files` lists. Returns `true` when the copy
    /// was newly attached, `false` when it was already recorded or the
    /// point no longer exists.
    pub async fn attach_copy(
        &self,
        id: Uuid,
        machine: Option<&str>,
        file: Option<&str>,
        repo: Option<&str>,
    ) -> StoreResult<bool> {
        let _guard = self.bookkeeping.lock().await;
        let resp = self
            .client
            .get_points(
                qdrant_client::qdrant::GetPointsBuilder::new(
                    self.collection.clone(),
                    vec![PointId {
                        point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
                    }],
                )
                .with_payload(true)
                .with_vectors(false),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant attach_copy get: {e}")))?;
        let Some(point) = resp.result.into_iter().next() else {
            return Ok(false);
        };
        let mut copies = parse_copies(&point.payload);
        let new = CopyEntry {
            machine: machine.map(str::to_owned),
            file: file.map(str::to_owned),
            repo: repo.map(str::to_owned),
        };
        if copies
            .iter()
            .any(|c| c.machine == new.machine && c.file == new.file)
        {
            return Ok(false);
        }
        copies.push(new);
        self.write_copies(id, &copies).await?;
        Ok(true)
    }

    /// Write the copy set for `id`: the `copies` list, the derived
    /// `machines`/`files` lists, and the canonical singular
    /// `machine`/`file`/`repo` promoted from the first copy.
    async fn write_copies(&self, id: Uuid, copies: &[CopyEntry]) -> StoreResult<()> {
        let selector = PointsSelectorOneOf::Points(PointsIdsList {
            ids: vec![PointId {
                point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
            }],
        });
        let (payload, cleared) = copies_payload(copies);
        self.client
            .set_payload(
                SetPayloadPointsBuilder::new(self.collection.clone(), payload)
                    .points_selector(selector.clone())
                    .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant write_copies set: {e}")))?;
        if !cleared.is_empty() {
            self.client
                .delete_payload(
                    qdrant_client::qdrant::DeletePayloadPointsBuilder::new(
                        self.collection.clone(),
                        cleared,
                    )
                    .points_selector(selector)
                    .wait(true),
                )
                .await
                .map_err(|e| StoreError::Backend(format!("qdrant write_copies clear: {e}")))?;
        }
        Ok(())
    }

    pub async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        let resp = self
            .client
            .get_points(
                qdrant_client::qdrant::GetPointsBuilder::new(
                    self.collection.clone(),
                    vec![PointId {
                        point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
                    }],
                )
                .with_payload(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant get_points: {e}")))?;
        Ok(resp
            .result
            .into_iter()
            .next()
            .and_then(|p| payload_to_item(&p.payload)))
    }

    /// Cheap liveness probe: list collections.
    pub async fn health(&self) -> StoreResult<()> {
        self.client
            .list_collections()
            .await
            .map(|_| ())
            .map_err(|e| StoreError::Backend(format!("qdrant health: {e}")))
    }

    /// Remove the (`machine`, `source_file`) copy from every knowledge
    /// point that carries it (sprint 028 #642). A point is deleted only
    /// when its last copy goes; otherwise its `copies` bookkeeping is
    /// rewritten and the canonical fields re-promoted. Points written
    /// before 028 carry no `copies` list — their singular
    /// `machine`/`file` is treated as their only copy, so they hard-
    /// delete exactly as before. Returns the number of copies removed
    /// (= points affected).
    ///
    /// `machine: None` is the legacy unscoped path (pre-025 semantics,
    /// unreachable from the API): every point whose `file` matches is
    /// hard-deleted outright.
    pub async fn delete_by_source_file(
        &self,
        source_file: &str,
        machine: Option<&str>,
    ) -> StoreResult<u64> {
        let Some(host) = machine else {
            return self
                .hard_delete_by_filter(Filter::must(vec![Condition::matches(
                    "file",
                    source_file.to_string(),
                )]))
                .await;
        };

        let _guard = self.bookkeeping.lock().await;
        // Two candidate shapes: post-028 points index every copy in the
        // `machines`/`files` lists; pre-028 points only have the
        // singular fields. Either filter can over-match (a point may
        // list the host and the file via *different* copies), so the
        // authoritative check is on the parsed copies below.
        let filters = [
            Filter::must(vec![
                Condition::matches("machines", host.to_string()),
                Condition::matches("files", source_file.to_string()),
            ]),
            Filter::must(vec![
                Condition::matches("machine", host.to_string()),
                Condition::matches("file", source_file.to_string()),
            ]),
        ];
        let mut candidates: Vec<(Uuid, Vec<CopyEntry>)> = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for filter in filters {
            let mut offset: Option<PointId> = None;
            loop {
                let mut builder = ScrollPointsBuilder::new(self.collection.clone())
                    .filter(filter.clone())
                    .limit(256)
                    .with_payload(true)
                    .with_vectors(false);
                if let Some(o) = offset.clone() {
                    builder = builder.offset(o);
                }
                let page = self
                    .client
                    .scroll(builder)
                    .await
                    .map_err(|e| StoreError::Backend(format!("qdrant scroll: {e}")))?;
                for p in &page.result {
                    let Some(PointIdOptions::Uuid(s)) =
                        p.id.as_ref().and_then(|i| i.point_id_options.as_ref())
                    else {
                        continue;
                    };
                    let Ok(id) = Uuid::parse_str(s) else { continue };
                    if seen_ids.insert(id) {
                        candidates.push((id, parse_copies(&p.payload)));
                    }
                }
                if page.next_page_offset.is_none() {
                    break;
                }
                offset = page.next_page_offset;
            }
        }

        let mut removed: u64 = 0;
        for (id, copies) in candidates {
            let remaining: Vec<CopyEntry> = copies
                .iter()
                .filter(|c| {
                    !(c.machine.as_deref() == Some(host) && c.file.as_deref() == Some(source_file))
                })
                .cloned()
                .collect();
            if remaining.len() == copies.len() {
                continue; // matched the filter via distinct copies; not this (host, file)
            }
            if remaining.is_empty() {
                self.hard_delete_point(id).await?;
            } else {
                self.write_copies(id, &remaining).await?;
            }
            removed += 1;
        }
        Ok(removed)
    }

    /// Hard-delete every point matching `filter`, returning the count
    /// via a scroll-then-delete round trip (Qdrant's delete RPC reports
    /// no count).
    async fn hard_delete_by_filter(&self, filter: Filter) -> StoreResult<u64> {
        let mut deleted: u64 = 0;
        let mut offset: Option<PointId> = None;
        loop {
            let mut builder = ScrollPointsBuilder::new(self.collection.clone())
                .filter(filter.clone())
                .limit(256)
                .with_payload(false)
                .with_vectors(false);
            if let Some(o) = offset.clone() {
                builder = builder.offset(o);
            }
            let page = self
                .client
                .scroll(builder)
                .await
                .map_err(|e| StoreError::Backend(format!("qdrant scroll: {e}")))?;
            deleted += page.result.len() as u64;
            if page.next_page_offset.is_none() {
                break;
            }
            offset = page.next_page_offset;
        }
        if deleted == 0 {
            return Ok(0);
        }
        self.client
            .delete_points(
                DeletePointsBuilder::new(self.collection.clone())
                    .points(filter)
                    .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant delete: {e}")))?;
        Ok(deleted)
    }
}

/// One recorded location of a point's content (sprint 028 #642).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CopyEntry {
    machine: Option<String>,
    file: Option<String>,
    repo: Option<String>,
}

fn copy_to_value(c: &CopyEntry) -> Value {
    let mut fields = HashMap::new();
    if let Some(m) = &c.machine {
        fields.insert("machine".to_string(), Value::from(m.clone()));
    }
    if let Some(f) = &c.file {
        fields.insert("file".to_string(), Value::from(f.clone()));
    }
    if let Some(r) = &c.repo {
        fields.insert("repo".to_string(), Value::from(r.clone()));
    }
    Value {
        kind: Some(ValueKind::StructValue(qdrant_client::qdrant::Struct {
            fields,
        })),
    }
}

fn value_to_copy(v: &Value) -> Option<CopyEntry> {
    let Some(ValueKind::StructValue(s)) = &v.kind else {
        return None;
    };
    let get = |k: &str| s.fields.get(k).and_then(|v| v.as_str().cloned());
    Some(CopyEntry {
        machine: get("machine"),
        file: get("file"),
        repo: get("repo"),
    })
}

/// Parse a point's copy set. A point written before 028 has no `copies`
/// key — its singular `machine`/`file`/`repo` is its only copy. A point
/// with neither (an agent memory) has none.
fn parse_copies(payload: &HashMap<String, Value>) -> Vec<CopyEntry> {
    if let Some(Value {
        kind: Some(ValueKind::ListValue(lv)),
    }) = payload.get("copies")
    {
        return lv.values.iter().filter_map(value_to_copy).collect();
    }
    let single = CopyEntry {
        machine: payload.get("machine").and_then(|v| v.as_str().cloned()),
        file: payload.get("file").and_then(|v| v.as_str().cloned()),
        repo: payload.get("repo").and_then(|v| v.as_str().cloned()),
    };
    if single.machine.is_some() || single.file.is_some() {
        vec![single]
    } else {
        Vec::new()
    }
}

/// Payload for a copy set: the `copies` list, derived unique
/// `machines`/`files` lists, and the canonical singular
/// `machine`/`file`/`repo` promoted from the first copy. Returns the
/// payload to set plus the singular keys that must be *cleared* because
/// the promoted copy has no value for them (`set_payload` merges; it
/// cannot remove).
fn copies_payload(copies: &[CopyEntry]) -> (HashMap<String, Value>, Vec<String>) {
    let mut p = HashMap::new();
    let mut cleared = Vec::new();
    p.insert(
        "copies".to_string(),
        Value {
            kind: Some(ValueKind::ListValue(ListValue {
                values: copies.iter().map(copy_to_value).collect(),
            })),
        },
    );
    let mut machines: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for c in copies {
        if let Some(m) = &c.machine {
            if !machines.contains(m) {
                machines.push(m.clone());
            }
        }
        if let Some(f) = &c.file {
            if !files.contains(f) {
                files.push(f.clone());
            }
        }
    }
    let str_list = |xs: &[String]| Value {
        kind: Some(ValueKind::ListValue(ListValue {
            values: xs.iter().map(|x| Value::from(x.clone())).collect(),
        })),
    };
    p.insert("machines".to_string(), str_list(&machines));
    p.insert("files".to_string(), str_list(&files));
    let canonical = copies.first();
    for (key, val) in [
        ("machine", canonical.and_then(|c| c.machine.clone())),
        ("file", canonical.and_then(|c| c.file.clone())),
        ("repo", canonical.and_then(|c| c.repo.clone())),
    ] {
        match val {
            Some(v) => {
                p.insert(key.to_string(), Value::from(v));
            }
            None => cleared.push(key.to_string()),
        }
    }
    (p, cleared)
}

fn item_to_payload(item: &KnowledgeItem) -> HashMap<String, Value> {
    let mut p = HashMap::new();
    p.insert("id".into(), Value::from(item.id.to_string()));
    p.insert("text".into(), Value::from(item.text.clone()));
    p.insert(
        "content_hash".into(),
        Value::from(item.content_hash.clone()),
    );
    p.insert(
        "source".into(),
        Value::from(item.source.as_str().to_string()),
    );
    p.insert(
        "tags".into(),
        Value {
            kind: Some(ValueKind::ListValue(ListValue {
                values: item.tags.iter().map(|t| Value::from(t.clone())).collect(),
            })),
        },
    );
    if let Some(v) = &item.repo {
        p.insert("repo".into(), Value::from(v.clone()));
    }
    if let Some(v) = &item.file {
        p.insert("file".into(), Value::from(v.clone()));
    }
    if let Some(v) = &item.machine {
        p.insert("machine".into(), Value::from(v.clone()));
    }
    p.insert("confidence".into(), Value::from(f64::from(item.confidence)));
    p.insert(
        "decay_weight".into(),
        Value::from(f64::from(item.decay_weight)),
    );
    p.insert("use_count".into(), Value::from(item.use_count));
    // Sprint 029 (#638): lifecycle fields, written only when present so
    // scanner points and pre-029 memories carry nothing extra.
    if let Some(v) = &item.volatility {
        p.insert("volatility".into(), Value::from(v.clone()));
    }
    if let Some(v) = &item.supersedes {
        p.insert("supersedes".into(), Value::from(v.to_string()));
    }
    if let Some(v) = &item.superseded_by {
        p.insert("superseded_by".into(), Value::from(v.to_string()));
    }
    p.insert(
        "created_at".into(),
        Value::from(item.created_at.format(&Rfc3339).unwrap_or_default()),
    );
    p.insert(
        "updated_at".into(),
        Value::from(item.updated_at.format(&Rfc3339).unwrap_or_default()),
    );
    p
}

#[allow(clippy::cast_possible_truncation)] // qdrant stores f64; KnowledgeItem stores f32
fn payload_to_item(payload: &HashMap<String, Value>) -> Option<KnowledgeItem> {
    let id = Uuid::parse_str(payload.get("id")?.as_str()?).ok()?;
    let text = payload.get("text")?.as_str()?.clone();
    let content_hash = payload.get("content_hash")?.as_str()?.clone();
    let source_str = payload.get("source")?.as_str()?.as_str();
    let source = match source_str {
        "User" => Source::User,
        "Controller" => Source::Controller,
        "Task" => Source::Task,
        "AgentProposal" => Source::AgentProposal,
        _ => return None,
    };
    let tags = payload
        .get("tags")
        .and_then(|v| match &v.kind {
            Some(ValueKind::ListValue(lv)) => Some(
                lv.values
                    .iter()
                    .filter_map(|x| x.as_str().cloned())
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    let created_at = payload
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);
    let updated_at = payload
        .get("updated_at")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or(created_at);
    Some(KnowledgeItem {
        id,
        text,
        content_hash,
        source,
        tags,
        repo: payload.get("repo").and_then(|v| v.as_str().cloned()),
        file: payload.get("file").and_then(|v| v.as_str().cloned()),
        machine: payload.get("machine").and_then(|v| v.as_str().cloned()),
        // Sprint 028 (#642): every host holding a copy. Pre-028 points
        // and agent memories have no list — empty on the way out.
        machines: payload
            .get("machines")
            .and_then(|v| match &v.kind {
                Some(ValueKind::ListValue(lv)) => Some(
                    lv.values
                        .iter()
                        .filter_map(|x| x.as_str().cloned())
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default(),
        // Sprint 026 (#641): these three have been written to the
        // payload since sprint 022 but were never read back, so no read
        // path could project them. Optional on the way out — points
        // written before 022, and non-chunked agent memories, have none.
        heading_path: payload
            .get("heading_path")
            .and_then(|v| v.as_str().cloned()),
        language: payload.get("language").and_then(|v| v.as_str().cloned()),
        chunk_index: payload
            .get("chunk_index")
            .and_then(qdrant_client::qdrant::Value::as_integer)
            .and_then(|i| u32::try_from(i).ok()),
        // Sprint 029 (#638): lifecycle fields. Absent on scanner points
        // and pre-029 memories.
        volatility: payload.get("volatility").and_then(|v| v.as_str().cloned()),
        supersedes: payload
            .get("supersedes")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        superseded_by: payload
            .get("superseded_by")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
        confidence: payload
            .get("confidence")
            .and_then(qdrant_client::qdrant::Value::as_double)
            .map_or(1.0, |d| d as f32),
        decay_weight: payload
            .get("decay_weight")
            .and_then(qdrant_client::qdrant::Value::as_double)
            .map_or(1.0, |d| d as f32),
        use_count: payload
            .get("use_count")
            .and_then(qdrant_client::qdrant::Value::as_integer)
            .unwrap_or(0),
        last_used_at: None,
        created_at,
        updated_at,
    })
}

// ---------- sprint 007: author attribution + soft-delete ----------

impl QdrantStore {
    /// Soft-delete a knowledge point by stamping `deleted_at` (RFC-3339)
    /// and `deleted_by_author_id` (UUID string) into its payload. The
    /// vector and other fields are untouched. Returns `Ok(())` on
    /// success even if the point was already soft-deleted; the default
    /// search filter (`is_empty("deleted_at")`) hides it either way.
    pub async fn soft_delete_payload(
        &self,
        id: uuid::Uuid,
        by_author_id: uuid::Uuid,
        when: OffsetDateTime,
    ) -> StoreResult<()> {
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "deleted_at".to_string(),
            Value::from(when.format(&Rfc3339).unwrap_or_default()),
        );
        payload.insert(
            "deleted_by_author_id".to_string(),
            Value::from(by_author_id.to_string()),
        );
        let points = PointsIdsList {
            ids: vec![PointId {
                point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
            }],
        };
        self.client
            .set_payload(
                SetPayloadPointsBuilder::new(self.collection.clone(), payload)
                    .points_selector(points)
                    .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant soft_delete_payload: {e}")))?;
        Ok(())
    }

    /// Mark a knowledge point superseded (sprint 029, #638): stamps the
    /// soft-delete pair — `deleted_at` + `deleted_by_author_id` — so
    /// every existing retrieval filter hides it, plus `superseded_by`
    /// pointing at the replacement. Supersession *is* the soft-delete
    /// mechanics with a pointer; `memory_admin_restore` undoes the
    /// hiding, and the pointer distinguishes "superseded" from
    /// "deleted" on the admin surface.
    pub async fn mark_superseded(
        &self,
        old_id: uuid::Uuid,
        new_id: uuid::Uuid,
        by_author_id: uuid::Uuid,
        when: OffsetDateTime,
    ) -> StoreResult<()> {
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "deleted_at".to_string(),
            Value::from(when.format(&Rfc3339).unwrap_or_default()),
        );
        payload.insert(
            "deleted_by_author_id".to_string(),
            Value::from(by_author_id.to_string()),
        );
        payload.insert("superseded_by".to_string(), Value::from(new_id.to_string()));
        let points = PointsIdsList {
            ids: vec![PointId {
                point_id_options: Some(PointIdOptions::Uuid(old_id.to_string())),
            }],
        };
        self.client
            .set_payload(
                SetPayloadPointsBuilder::new(self.collection.clone(), payload)
                    .points_selector(points)
                    .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant mark_superseded: {e}")))?;
        Ok(())
    }

    /// Rewrite an existing knowledge point in place (sprint 029,
    /// `memory_update`): full payload rebuild from `item` plus the
    /// (unchanged) `author_id`, with the supplied embedding. The point
    /// id stays `item.id`, so this is an upsert-as-update. Only for
    /// agent-authored memories — their payloads round-trip losslessly
    /// through `KnowledgeItem` (no `symbols`, no copy bookkeeping);
    /// scanner chunks do not, and their update path is the re-scan.
    pub async fn upsert_knowledge_item(
        &self,
        item: &KnowledgeItem,
        author_id: uuid::Uuid,
        embedding: Vec<f32>,
    ) -> StoreResult<()> {
        if embedding.len() as u64 != self.vector_dim {
            return Err(StoreError::Other(format!(
                "embedding dim {} != collection dim {}",
                embedding.len(),
                self.vector_dim
            )));
        }
        let mut payload = item_to_payload(item);
        payload.insert("author_id".into(), Value::from(author_id.to_string()));
        let point = PointStruct::new(
            PointId {
                point_id_options: Some(PointIdOptions::Uuid(item.id.to_string())),
            },
            embedding,
            payload,
        );
        self.client
            .upsert_points(
                UpsertPointsBuilder::new(self.collection.clone(), vec![point]).wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant upsert_knowledge_item: {e}")))?;
        Ok(())
    }

    /// Remove the soft-delete payload fields, returning the point to live
    /// state. No-op if the point is not soft-deleted.
    pub async fn restore_payload(&self, id: uuid::Uuid) -> StoreResult<()> {
        use qdrant_client::qdrant::DeletePayloadPointsBuilder;
        let points = PointsIdsList {
            ids: vec![PointId {
                point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
            }],
        };
        self.client
            .delete_payload(
                DeletePayloadPointsBuilder::new(
                    self.collection.clone(),
                    vec!["deleted_at".to_string(), "deleted_by_author_id".to_string()],
                )
                .points_selector(points)
                .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant restore_payload: {e}")))?;
        Ok(())
    }

    /// Fetch the embedding vector for a single knowledge point. Returns
    /// `None` if the point is unknown or has no vector. Used by
    /// `memory_related` to seed a nearest-neighbour search from an
    /// existing memory id.
    pub async fn get_point_vector(&self, id: uuid::Uuid) -> StoreResult<Option<Vec<f32>>> {
        use qdrant_client::qdrant::{vector_output::Vector as VectorKind, GetPointsBuilder};
        let resp = self
            .client
            .get_points(
                GetPointsBuilder::new(
                    self.collection.clone(),
                    vec![PointId {
                        point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
                    }],
                )
                .with_payload(false)
                .with_vectors(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant get_point_vector: {e}")))?;
        Ok(resp
            .result
            .into_iter()
            .next()
            .and_then(|p| p.vectors)
            .and_then(|v| v.get_vector())
            .and_then(|v| match v {
                VectorKind::Dense(d) => Some(d.data),
                VectorKind::Sparse(_) | VectorKind::MultiDense(_) => None,
            }))
    }

    /// Bulk-lookup `author_id` payload values for a set of knowledge
    /// point ids. Missing ids and points without an `author_id` field
    /// are simply absent from the returned map.
    pub async fn knowledge_authors_by_ids(
        &self,
        ids: &[uuid::Uuid],
    ) -> StoreResult<HashMap<uuid::Uuid, uuid::Uuid>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let point_ids: Vec<PointId> = ids
            .iter()
            .map(|id| PointId {
                point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
            })
            .collect();
        let resp = self
            .client
            .get_points(
                qdrant_client::qdrant::GetPointsBuilder::new(self.collection.clone(), point_ids)
                    .with_payload(true)
                    .with_vectors(false),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant knowledge_authors_by_ids: {e}")))?;
        let mut out = HashMap::with_capacity(resp.result.len());
        for p in resp.result {
            let Some(point_id) = p.id.and_then(|p| p.point_id_options) else {
                continue;
            };
            let PointIdOptions::Uuid(s) = point_id else {
                continue;
            };
            let Ok(pid) = Uuid::parse_str(&s) else {
                continue;
            };
            if let Some(author) = p
                .payload
                .get("author_id")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                out.insert(pid, author);
            }
        }
        Ok(out)
    }

    /// Returns `true` if a point with `id` exists in the collection,
    /// regardless of its soft-delete state. Used by `memory_delete` to
    /// distinguish `NOT_FOUND` from "already soft-deleted" (FR-014).
    pub async fn point_exists_any(&self, id: uuid::Uuid) -> StoreResult<bool> {
        let resp = self
            .client
            .get_points(
                qdrant_client::qdrant::GetPointsBuilder::new(
                    self.collection.clone(),
                    vec![PointId {
                        point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
                    }],
                )
                .with_payload(false)
                .with_vectors(false),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant point_exists_any: {e}")))?;
        Ok(!resp.result.is_empty())
    }

    /// Returns `Some(true)` if the point exists AND has a non-empty
    /// `deleted_at` payload field, `Some(false)` if it exists but is
    /// live, and `None` if the point is unknown. Used by
    /// `memory_admin_restore` to distinguish `NOT_SOFT_DELETED` from
    /// `NOT_FOUND`.
    pub async fn point_is_soft_deleted(&self, id: uuid::Uuid) -> StoreResult<Option<bool>> {
        let resp = self
            .client
            .get_points(
                qdrant_client::qdrant::GetPointsBuilder::new(
                    self.collection.clone(),
                    vec![PointId {
                        point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
                    }],
                )
                .with_payload(true)
                .with_vectors(false),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant point_is_soft_deleted: {e}")))?;
        let Some(p) = resp.result.into_iter().next() else {
            return Ok(None);
        };
        let deleted = p
            .payload
            .get("deleted_at")
            .and_then(qdrant_client::qdrant::Value::as_str)
            .is_some_and(|s| !s.is_empty());
        Ok(Some(deleted))
    }

    /// Scroll the collection for soft-deleted knowledge points
    /// (payload `deleted_at` set). Returns up to `limit` items plus
    /// the next scroll offset (opaque cursor) when more pages remain.
    /// Optional `author_id` filter narrows to a single deleter.
    pub async fn list_deleted_knowledge(
        &self,
        limit: u32,
        author_id: Option<uuid::Uuid>,
        offset: Option<uuid::Uuid>,
    ) -> StoreResult<(
        Vec<(
            KnowledgeItem,
            OffsetDateTime,
            Option<uuid::Uuid>,
            Option<uuid::Uuid>,
        )>,
        Option<uuid::Uuid>,
    )> {
        let mut must: Vec<Condition> = Vec::new();
        if let Some(a) = author_id {
            must.push(Condition::matches("deleted_by_author_id", a.to_string()));
        }
        let filter = Filter {
            must,
            must_not: vec![Condition::is_empty("deleted_at")],
            ..Default::default()
        };
        let mut builder = ScrollPointsBuilder::new(self.collection.clone())
            .filter(filter)
            .limit(limit.clamp(1, 500))
            .with_payload(true)
            .with_vectors(false);
        if let Some(o) = offset {
            builder = builder.offset(PointId {
                point_id_options: Some(PointIdOptions::Uuid(o.to_string())),
            });
        }
        let resp = self
            .client
            .scroll(builder)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant scroll deleted: {e}")))?;
        let next = resp
            .next_page_offset
            .and_then(|p| match p.point_id_options? {
                PointIdOptions::Uuid(s) => uuid::Uuid::parse_str(&s).ok(),
                PointIdOptions::Num(_) => None,
            });
        let mut out = Vec::with_capacity(resp.result.len());
        for p in resp.result {
            let Some(item) = payload_to_item(&p.payload) else {
                continue;
            };
            let deleted_at = p
                .payload
                .get("deleted_at")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
                .unwrap_or_else(OffsetDateTime::now_utc);
            let deleter = p
                .payload
                .get("deleted_by_author_id")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| uuid::Uuid::parse_str(s).ok());
            let author = p
                .payload
                .get("author_id")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| uuid::Uuid::parse_str(s).ok());
            out.push((item, deleted_at, deleter, author));
        }
        Ok((out, next))
    }

    /// Sprint 009 (kwi #32): count live knowledge points authored by
    /// `author_id`. Used by `/v1/authors/:id` so the per-author detail
    /// view can show a `knowledge` write count alongside facts/events.
    /// Uses Qdrant's exact-count endpoint with a payload filter on
    /// `author_id` + `is_empty(deleted_at)`.
    pub async fn count_live_knowledge_by_author(&self, author_id: uuid::Uuid) -> StoreResult<u64> {
        let filter = Filter {
            must: vec![
                Condition::matches("author_id", author_id.to_string()),
                Condition::is_empty("deleted_at"),
            ],
            ..Default::default()
        };
        let resp = self
            .client
            .count(
                CountPointsBuilder::new(self.collection.clone())
                    .filter(filter)
                    .exact(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant count by_author: {e}")))?;
        Ok(resp.result.map_or(0, |r| r.count))
    }

    /// Sprint 025 (#636) — count knowledge points authored by
    /// `author_id` in **any** state, soft-deleted included. Removing an
    /// author must be blocked by a soft-deleted point just as much as a
    /// live one: the point still carries the attribution, and dropping
    /// the author row would orphan it.
    pub async fn count_knowledge_by_author_any(&self, author_id: uuid::Uuid) -> StoreResult<u64> {
        let filter = Filter {
            must: vec![Condition::matches("author_id", author_id.to_string())],
            ..Default::default()
        };
        let resp = self
            .client
            .count(
                CountPointsBuilder::new(self.collection.clone())
                    .filter(filter)
                    .exact(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant count_by_author_any: {e}")))?;
        Ok(resp.result.map_or(0, |r| r.count))
    }

    /// Sprint 025 (#636) — repoint every knowledge point authored by
    /// `from` at `into`, for the merge path. Qdrant has no
    /// transactions, so this is the step the merge runs **first**: if it
    /// fails, nothing in Postgres has changed and the merge is simply
    /// re-runnable.
    ///
    /// Returns the number of points repointed.
    pub async fn reassign_knowledge_author(
        &self,
        from: uuid::Uuid,
        into: uuid::Uuid,
    ) -> StoreResult<u64> {
        let moved = self.count_knowledge_by_author_any(from).await?;
        if moved == 0 {
            return Ok(0);
        }
        let filter = Filter {
            must: vec![Condition::matches("author_id", from.to_string())],
            ..Default::default()
        };
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "author_id".to_string(),
            qdrant_client::qdrant::Value::from(into.to_string()),
        );
        self.client
            .set_payload(
                qdrant_client::qdrant::SetPayloadPointsBuilder::new(
                    self.collection.clone(),
                    payload,
                )
                .points_selector(filter)
                .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant reassign_knowledge_author: {e}")))?;
        Ok(moved)
    }

    /// Scroll the collection for knowledge points authored by
    /// `author_id`. `state` selects live vs soft-deleted vs all.
    /// Returns up to `limit` items plus the next scroll offset.
    pub async fn list_knowledge_by_author(
        &self,
        author_id: uuid::Uuid,
        state: AuthorMemoryStateFilter,
        limit: u32,
        offset: Option<uuid::Uuid>,
    ) -> StoreResult<(
        Vec<(KnowledgeItem, Option<OffsetDateTime>, Option<uuid::Uuid>)>,
        Option<uuid::Uuid>,
    )> {
        let mut must: Vec<Condition> = vec![Condition::matches("author_id", author_id.to_string())];
        let mut must_not: Vec<Condition> = Vec::new();
        match state {
            AuthorMemoryStateFilter::Live => must.push(Condition::is_empty("deleted_at")),
            AuthorMemoryStateFilter::Deleted => must_not.push(Condition::is_empty("deleted_at")),
            AuthorMemoryStateFilter::All => {}
        }
        let filter = Filter {
            must,
            must_not,
            ..Default::default()
        };
        let mut builder = ScrollPointsBuilder::new(self.collection.clone())
            .filter(filter)
            .limit(limit.clamp(1, 500))
            .with_payload(true)
            .with_vectors(false);
        if let Some(o) = offset {
            builder = builder.offset(PointId {
                point_id_options: Some(PointIdOptions::Uuid(o.to_string())),
            });
        }
        let resp = self
            .client
            .scroll(builder)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant scroll by_author: {e}")))?;
        let next = resp
            .next_page_offset
            .and_then(|p| match p.point_id_options? {
                PointIdOptions::Uuid(s) => uuid::Uuid::parse_str(&s).ok(),
                PointIdOptions::Num(_) => None,
            });
        let mut out = Vec::with_capacity(resp.result.len());
        for p in resp.result {
            let Some(item) = payload_to_item(&p.payload) else {
                continue;
            };
            let deleted_at = p
                .payload
                .get("deleted_at")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
            let deleter = p
                .payload
                .get("deleted_by_author_id")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| uuid::Uuid::parse_str(s).ok());
            out.push((item, deleted_at, deleter));
        }
        Ok((out, next))
    }

    /// Sprint 008 / #54 — cross-author page for `GET /v1/memories`,
    /// **newest-first**. Filters by `authors` (empty ⇒ all) and `state`, and
    /// orders by the `created_at` datetime payload index (created in `connect`)
    /// so the page is `created_at DESC` instead of point-id order (which is
    /// oldest-first and put new knowledge at the bottom of the feed).
    ///
    /// `cursor` is the `(created_at, id)` keyset of the last row the composite
    /// merge emitted (or `None` for the first page). The Qdrant scan starts at
    /// that timestamp; the exact keyset and the `[since, until)` window are
    /// enforced client-side on the RFC3339 payload (`start_from` is inclusive and
    /// millisecond-granular). Returns rows `created_at DESC` plus the keyset of
    /// the last row when a further page may exist.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_memories_knowledge_page(
        &self,
        authors: &[uuid::Uuid],
        state: AuthorMemoryStateFilter,
        since: chrono::DateTime<chrono::Utc>,
        until: chrono::DateTime<chrono::Utc>,
        limit: u32,
        cursor: Option<(OffsetDateTime, uuid::Uuid)>,
    ) -> StoreResult<(
        Vec<(
            KnowledgeItem,
            uuid::Uuid,
            Option<OffsetDateTime>,
            Option<uuid::Uuid>,
        )>,
        Option<(OffsetDateTime, uuid::Uuid)>,
    )> {
        use qdrant_client::qdrant::{start_from, Direction, OrderBy, StartFrom};

        let mut must: Vec<Condition> = Vec::new();
        let mut must_not: Vec<Condition> = Vec::new();
        let should: Vec<Condition> = authors
            .iter()
            .map(|a| Condition::matches("author_id", a.to_string()))
            .collect();
        match state {
            AuthorMemoryStateFilter::Live => must.push(Condition::is_empty("deleted_at")),
            AuthorMemoryStateFilter::Deleted => must_not.push(Condition::is_empty("deleted_at")),
            AuthorMemoryStateFilter::All => {}
        }
        let filter = Filter {
            must,
            must_not,
            should,
            ..Default::default()
        };

        let since_off = chrono_to_offset(since);
        let until_off = chrono_to_offset(until);
        // Descend from the cursor timestamp (or the window's upper bound on the
        // first page); the precise keyset/window are applied below.
        let start = cursor.map_or(until_off, |(ts, _)| ts);
        let order_by = OrderBy {
            key: "created_at".to_string(),
            direction: Some(Direction::Desc as i32),
            start_from: Some(StartFrom {
                value: Some(start_from::Value::Datetime(
                    start.format(&Rfc3339).unwrap_or_default(),
                )),
            }),
        };
        // Over-fetch to absorb the client-side window + keyset filtering.
        let scroll_limit = limit.saturating_mul(4).clamp(1, 500);
        let builder = ScrollPointsBuilder::new(self.collection.clone())
            .filter(filter)
            .order_by(order_by)
            .limit(scroll_limit)
            .with_payload(true)
            .with_vectors(false);
        let resp = self
            .client
            .scroll(builder)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant scroll memories_page: {e}")))?;
        // If the raw scroll filled its cap, older rows may lie beyond it.
        let hit_cap = resp.result.len() >= scroll_limit as usize;
        let mut out = Vec::with_capacity(resp.result.len().min(limit as usize));
        let mut more = false;
        for p in resp.result {
            let Some(item) = payload_to_item(&p.payload) else {
                continue;
            };
            if item.created_at < since_off || item.created_at >= until_off {
                continue;
            }
            // Keyset: strictly older than the last row the merge already emitted
            // (`start_from` is inclusive, so drop the boundary point and any tie
            // with a higher id).
            if let Some((cts, cid)) = cursor {
                if (item.created_at, item.id) >= (cts, cid) {
                    continue;
                }
            }
            if out.len() == limit as usize {
                more = true;
                break;
            }
            let Some(author_id) = p
                .payload
                .get("author_id")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
            else {
                continue;
            };
            let deleted_at = p
                .payload
                .get("deleted_at")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
            let deleter = p
                .payload
                .get("deleted_by_author_id")
                .and_then(qdrant_client::qdrant::Value::as_str)
                .and_then(|s| uuid::Uuid::parse_str(s).ok());
            out.push((item, author_id, deleted_at, deleter));
        }
        // Offer a next keyset when this page is full or the scroll was capped;
        // the composite merge treats it only as a saturation hint.
        let next = if (more || hit_cap) && !out.is_empty() {
            out.last().map(|(item, _, _, _)| (item.created_at, item.id))
        } else {
            None
        };
        Ok((out, next))
    }

    /// Permanently remove a knowledge point.
    pub async fn hard_delete_point(&self, id: uuid::Uuid) -> StoreResult<()> {
        let points = PointsIdsList {
            ids: vec![PointId {
                point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
            }],
        };
        self.client
            .delete_points(
                DeletePointsBuilder::new(self.collection.clone())
                    .points(points)
                    .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant hard_delete_point: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Sprint 028 (#642) — copy bookkeeping helpers. One point per
    // content hash; per-(host, file) identity lives in `copies`.

    fn copy(machine: &str, file: &str, repo: &str) -> CopyEntry {
        CopyEntry {
            machine: Some(machine.into()),
            file: Some(file.into()),
            repo: Some(repo.into()),
        }
    }

    #[test]
    fn copies_round_trip_through_payload() {
        let copies = vec![
            copy("kubs0", "/src/a.md", "klams"),
            copy("kai", "/src/a.md", "klams"),
        ];
        let (payload, cleared) = copies_payload(&copies);
        assert!(cleared.is_empty());
        assert_eq!(parse_copies(&payload), copies);
    }

    #[test]
    fn machines_and_files_lists_are_unique_and_ordered() {
        let copies = vec![
            copy("kubs0", "/src/a.md", "klams"),
            copy("kai", "/src/a.md", "klams"),
            copy("kubs0", "/src/b.md", "klams"),
        ];
        let (payload, _) = copies_payload(&copies);
        let as_strs = |key: &str| -> Vec<String> {
            match &payload.get(key).unwrap().kind {
                Some(ValueKind::ListValue(lv)) => lv
                    .values
                    .iter()
                    .filter_map(|v| v.as_str().cloned())
                    .collect(),
                _ => panic!("{key} not a list"),
            }
        };
        assert_eq!(as_strs("machines"), vec!["kubs0", "kai"]);
        assert_eq!(as_strs("files"), vec!["/src/a.md", "/src/b.md"]);
    }

    #[test]
    fn canonical_singulars_promote_from_the_first_copy() {
        let copies = vec![
            copy("kai", "/x/b.rs", "krag"),
            copy("kubs0", "/x/a.rs", "klams"),
        ];
        let (payload, cleared) = copies_payload(&copies);
        assert_eq!(
            payload.get("machine").and_then(|v| v.as_str().cloned()),
            Some("kai".to_string())
        );
        assert_eq!(
            payload.get("file").and_then(|v| v.as_str().cloned()),
            Some("/x/b.rs".to_string())
        );
        assert_eq!(
            payload.get("repo").and_then(|v| v.as_str().cloned()),
            Some("krag".to_string())
        );
        assert!(cleared.is_empty());
    }

    #[test]
    fn empty_copy_set_clears_the_singular_fields() {
        // The last copy went away but the point survives only when
        // copies remain — still, the helper must say which singular keys
        // to clear rather than leave stale canonical fields behind.
        let (payload, cleared) = copies_payload(&[]);
        assert!(!payload.contains_key("machine"));
        assert_eq!(cleared, vec!["machine", "file", "repo"]);
    }

    #[test]
    fn pre_028_point_synthesizes_its_singular_fields_as_one_copy() {
        // Legacy points (one per host+file) have no `copies` list; their
        // singular fields are their only copy, so removing it deletes
        // the point exactly as the old semantics did.
        let mut payload = HashMap::new();
        payload.insert("machine".to_string(), Value::from("kai"));
        payload.insert("file".to_string(), Value::from("/src/x.rs"));
        payload.insert("repo".to_string(), Value::from("krag"));
        assert_eq!(
            parse_copies(&payload),
            vec![copy("kai", "/src/x.rs", "krag")]
        );
    }

    #[test]
    fn agent_memory_without_location_has_no_copies() {
        let mut payload = HashMap::new();
        payload.insert("text".to_string(), Value::from("a durable gotcha"));
        assert!(parse_copies(&payload).is_empty());
    }
}
