//! Search request/result shapes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchType {
    Fact,
    Event,
    Knowledge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(rename = "type")]
    pub kind: SearchType,
    pub id: Uuid,
    pub score: f32,
    pub preview: String,
    pub payload: serde_json::Value,
}
