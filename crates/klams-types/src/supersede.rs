//! Supersede proposals (sprint 058, klams-mind WI 3331).
//!
//! The propose-merge path Ken chose on 2026-10-23: an agent without the
//! authority to supersede another author's knowledge files a proposal;
//! an identity holding `manage` accepts or rejects it, and acceptance
//! performs the supersede. See `sprints/058-soft-delete-propose-supersede/`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// Lifecycle of a supersede proposal.
///
/// `pending` is the only live state. `stale` means acceptance was tried
/// and a named record was no longer live agent-authored knowledge — the
/// usual cause is a sibling proposal over the same cluster being
/// accepted first (decision 3: re-validate at acceptance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupersedeProposalStatus {
    Pending,
    Accepted,
    Rejected,
    Stale,
}

impl SupersedeProposalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Stale => "stale",
        }
    }

    /// Parse the stored spelling.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            "stale" => Self::Stale,
            _ => return None,
        })
    }
}

/// A proposal as filed, before the store assigns status and timestamps.
#[derive(Debug, Clone)]
pub struct NewSupersedeProposal {
    pub id: Uuid,
    pub author_id: Uuid,
    /// The record that survives (or, when `text` is set, the record the
    /// merged replacement supersedes).
    pub keeper_id: Uuid,
    /// Records retired into the survivor. Sorted and de-duplicated by
    /// the caller; never contains `keeper_id`.
    pub retire_ids: Vec<Uuid>,
    /// Merged replacement text. `None` = keep `keeper_id` as it stands.
    pub text: Option<String>,
    pub reason: String,
    /// Dedupe key over `(keeper_id, retire_ids, text)`.
    pub proposal_hash: String,
}

/// A stored supersede proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupersedeProposal {
    pub id: Uuid,
    pub author_id: Uuid,
    pub keeper_id: Uuid,
    pub retire_ids: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub reason: String,
    pub status: SupersedeProposalStatus,
    pub submission_count: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by_author_id: Option<Uuid>,
    /// The live record every retired id now points at (`superseded_by`):
    /// the keeper, or the replacement written from `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_memory_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_note: Option<String>,
}
