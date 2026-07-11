//! Qdrant adapter (knowledge items).

use crate::{StoreError, StoreResult};
use klams_types::{
    DigestCluster, IndexKnowledge, KnowledgeDigest, KnowledgeItem, Source, SummaryMechanism,
};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, points_selector::PointsSelectorOneOf, value::Kind as ValueKind,
    Condition, CountPointsBuilder, CreateCollectionBuilder, DeletePointsBuilder, Distance,
    FieldType, Filter, ListValue, PointId, PointStruct, PointsIdsList, ScrollPointsBuilder,
    SearchPointsBuilder, SetPayloadPointsBuilder, UpsertPointsBuilder, Value, VectorParamsBuilder,
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
}

impl std::fmt::Debug for QdrantStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QdrantStore")
            .field("client", &"<qdrant_client>")
            .field("collection", &self.collection)
            .field("vector_dim", &self.vector_dim)
            .finish()
    }
}

impl QdrantStore {
    /// Connect, ensure the `knowledge_items` collection and payload
    /// indexes exist.
    pub async fn connect(grpc_url: &str, collection: &str, vector_dim: u64) -> StoreResult<Self> {
        let client = Qdrant::from_url(grpc_url)
            .build()
            .map_err(|e| StoreError::Backend(format!("qdrant client: {e}")))?;

        let exists = client
            .collection_exists(collection)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant exists: {e}")))?;
        if !exists {
            client
                .create_collection(
                    CreateCollectionBuilder::new(collection)
                        .vectors_config(
                            VectorParamsBuilder::new(vector_dim, Distance::Cosine).on_disk(true),
                        )
                        .on_disk_payload(true),
                )
                .await
                .map_err(|e| StoreError::Backend(format!("qdrant create: {e}")))?;
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
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: now,
            updated_at: now,
        };
        let mut payload = item_to_payload(&item);
        payload.insert("author_id".into(), Value::from(req.author_id.to_string()));
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
        // Exclude digest points from the default raw-knowledge search
        // (sprint 005 T038). Existing knowledge items have no `kind`
        // payload field, so the `must_not` is a no-op for them.
        // Sprint 007: also exclude soft-deleted points (R-003) — they
        // are points whose payload carries a non-null `deleted_at`.
        let filter = Filter {
            must: vec![Condition::is_empty("deleted_at")],
            must_not: vec![Condition::matches("kind", "digest".to_string())],
            ..Default::default()
        };
        let resp = self
            .client
            .search_points(
                SearchPointsBuilder::new(self.collection.clone(), query_vector, u64::from(top_k))
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

    /// Upsert a knowledge digest point (sprint 005 T038). Writes a
    /// point with `kind = "digest"` in its payload so the default
    /// raw search ignores it. The caller supplies an embedding of
    /// the summary text.
    pub async fn upsert_digest(
        &self,
        digest: &KnowledgeDigest,
        embedding: Vec<f32>,
    ) -> StoreResult<()> {
        if embedding.len() as u64 != self.vector_dim {
            return Err(StoreError::Other(format!(
                "embedding dim {} != collection dim {}",
                embedding.len(),
                self.vector_dim
            )));
        }
        let payload = digest_to_payload(digest);
        let point = PointStruct::new(
            PointId {
                point_id_options: Some(PointIdOptions::Uuid(digest.id.to_string())),
            },
            embedding,
            payload,
        );
        self.client
            .upsert_points(
                UpsertPointsBuilder::new(self.collection.clone(), vec![point]).wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant upsert digest: {e}")))?;
        Ok(())
    }

    /// Mark a digest point invalidated (sprint 005 T038). Sets the
    /// `invalidated_at` payload field to "now"; subsequent calls to
    /// [`Self::search_digests`] will skip it.
    pub async fn invalidate_digest(&self, id: Uuid) -> StoreResult<()> {
        let mut payload = HashMap::new();
        payload.insert(
            "invalidated_at".into(),
            Value::from(
                OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .unwrap_or_default(),
            ),
        );
        let selector = PointsSelectorOneOf::Points(PointsIdsList {
            ids: vec![PointId {
                point_id_options: Some(PointIdOptions::Uuid(id.to_string())),
            }],
        });
        self.client
            .set_payload(
                SetPayloadPointsBuilder::new(self.collection.clone(), payload)
                    .points_selector(selector)
                    .wait(true),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant set_payload: {e}")))?;
        Ok(())
    }

    /// Vector search restricted to active digest points (sprint 005
    /// T038). Filters `kind = "digest"` and skips any point whose
    /// `invalidated_at` field is non-empty.
    pub async fn search_digests(
        &self,
        query_vector: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeDigest, f32)>> {
        let filter = Filter {
            must: vec![
                Condition::matches("kind", "digest".to_string()),
                Condition::is_empty("invalidated_at"),
            ],
            ..Default::default()
        };
        let resp = self
            .client
            .search_points(
                SearchPointsBuilder::new(self.collection.clone(), query_vector, u64::from(top_k))
                    .with_payload(true)
                    .filter(filter),
            )
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant search digest: {e}")))?;
        let mut out = Vec::with_capacity(resp.result.len());
        for sp in resp.result {
            if let Some(d) = payload_to_digest(&sp.payload) {
                out.push((d, sp.score));
            }
        }
        Ok(out)
    }

    pub async fn find_knowledge_by_content_hash(
        &self,
        hash: &str,
        source_file: Option<&str>,
    ) -> StoreResult<Option<Uuid>> {
        // Scope dedupe to the same source file (sprint 022 #324): an
        // identical chunk in two files must be two points, so deleting
        // one file can't drop a chunk still live in the other.
        let mut conds = vec![Condition::matches("content_hash", hash.to_string())];
        if let Some(f) = source_file {
            conds.push(Condition::matches("file", f.to_string()));
        }
        let filter = Filter::must(conds);
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

    /// Sprint 003 T010b: delete all knowledge points whose payload
    /// `source_file` matches `source_file`. Returns the count of
    /// deleted points via a `scroll`-then-`delete` round trip
    /// (Qdrant's `delete` RPC does not report a count).
    pub async fn delete_by_source_file(&self, source_file: &str) -> StoreResult<u64> {
        let filter = Filter::must([Condition::matches("file", source_file.to_string())]);

        // Count first (paged scroll, ids only).
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

fn digest_to_payload(d: &KnowledgeDigest) -> HashMap<String, Value> {
    let mut p = HashMap::new();
    p.insert("kind".into(), Value::from("digest"));
    p.insert("id".into(), Value::from(d.id.to_string()));
    p.insert("text".into(), Value::from(d.text.clone()));
    p.insert(
        "mechanism".into(),
        Value::from(d.mechanism.as_str().to_string()),
    );
    p.insert(
        "source_ids".into(),
        Value {
            kind: Some(ValueKind::ListValue(ListValue {
                values: d
                    .source_ids
                    .iter()
                    .map(|u| Value::from(u.to_string()))
                    .collect(),
            })),
        },
    );
    p.insert(
        "source_count".into(),
        Value::from(i64::from(d.source_count)),
    );
    p.insert("repo".into(), Value::from(d.cluster.repo.clone()));
    p.insert(
        "file_prefix".into(),
        Value::from(d.cluster.file_prefix.clone()),
    );
    p.insert(
        "generated_at".into(),
        Value::from(d.generated_at.format(&Rfc3339).unwrap_or_default()),
    );
    // `invalidated_at` is intentionally omitted when active so the
    // `is_empty` filter in `search_digests` matches.
    p
}

fn payload_to_digest(payload: &HashMap<String, Value>) -> Option<KnowledgeDigest> {
    if payload
        .get("kind")
        .and_then(|v| v.as_str())
        .map(String::as_str)
        != Some("digest")
    {
        return None;
    }
    let id = Uuid::parse_str(payload.get("id")?.as_str()?).ok()?;
    let text = payload.get("text")?.as_str()?.clone();
    let mechanism = match payload.get("mechanism")?.as_str()?.as_str() {
        "extractive" => SummaryMechanism::Extractive,
        "llm" => SummaryMechanism::Llm,
        _ => return None,
    };
    let source_ids: Vec<Uuid> = payload
        .get("source_ids")
        .and_then(|v| match &v.kind {
            Some(ValueKind::ListValue(lv)) => Some(
                lv.values
                    .iter()
                    .filter_map(|x| x.as_str().and_then(|s| Uuid::parse_str(s).ok()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    let source_count = u32::try_from(
        payload
            .get("source_count")
            .and_then(qdrant_client::qdrant::Value::as_integer)
            .unwrap_or(0),
    )
    .unwrap_or(0);
    let repo = payload
        .get("repo")
        .and_then(|v| v.as_str().cloned())
        .unwrap_or_default();
    let file_prefix = payload
        .get("file_prefix")
        .and_then(|v| v.as_str().cloned())
        .unwrap_or_default();
    let generated_at = payload
        .get("generated_at")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);
    let invalidated_at = payload
        .get("invalidated_at")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
    Some(KnowledgeDigest {
        id,
        text,
        mechanism,
        source_ids,
        source_count,
        cluster: DigestCluster { repo, file_prefix },
        generated_at,
        invalidated_at,
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
    use time::macros::datetime;

    fn sample_digest() -> KnowledgeDigest {
        KnowledgeDigest {
            id: Uuid::nil(),
            text: "summary text".into(),
            mechanism: SummaryMechanism::Extractive,
            source_ids: vec![Uuid::nil()],
            source_count: 7,
            cluster: DigestCluster {
                repo: "klams".into(),
                file_prefix: "docs/".into(),
            },
            generated_at: datetime!(2025-01-02 03:04:05 UTC),
            invalidated_at: None,
        }
    }

    #[test]
    fn digest_payload_round_trip() {
        let d = sample_digest();
        let payload = digest_to_payload(&d);
        assert_eq!(
            payload.get("kind").and_then(|v| v.as_str()),
            Some(&"digest".to_string())
        );
        assert!(!payload.contains_key("invalidated_at"));
        let back = payload_to_digest(&payload).expect("round-trip");
        assert_eq!(back.id, d.id);
        assert_eq!(back.text, d.text);
        assert_eq!(back.source_count, d.source_count);
        assert_eq!(back.cluster.repo, d.cluster.repo);
        assert_eq!(back.cluster.file_prefix, d.cluster.file_prefix);
        assert!(back.invalidated_at.is_none());
    }

    #[test]
    fn payload_without_kind_digest_is_rejected() {
        let mut payload = digest_to_payload(&sample_digest());
        payload.remove("kind");
        assert!(payload_to_digest(&payload).is_none());
    }
}
