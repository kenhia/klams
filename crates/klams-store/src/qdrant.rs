//! Qdrant adapter (knowledge items).

use crate::{StoreError, StoreResult};
use klams_types::{IndexKnowledge, KnowledgeItem, Source};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, value::Kind as ValueKind, Condition, CreateCollectionBuilder,
    Distance, FieldType, Filter, ListValue, PointId, PointStruct, ScrollPointsBuilder,
    SearchPointsBuilder, UpsertPointsBuilder, Value, VectorParamsBuilder,
};
use qdrant_client::Qdrant;
use std::collections::HashMap;
use std::sync::Arc;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;

const KEYWORD_INDEX_FIELDS: &[&str] = &["content_hash", "source", "tags", "repo", "machine"];

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
        }

        Ok(Self {
            client: Arc::new(client),
            collection: collection.to_string(),
            vector_dim,
        })
    }

    pub fn collection(&self) -> &str {
        &self.collection
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
        let payload = item_to_payload(&item);
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
        let resp = self
            .client
            .search_points(
                SearchPointsBuilder::new(self.collection.clone(), query_vector, u64::from(top_k))
                    .with_payload(true),
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

    pub async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>> {
        let filter = Filter::must([Condition::matches("content_hash", hash.to_string())]);
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
