//! `supersede_proposals_list` MCP tool (sprint 058, klams-mind WI 3331).
//!
//! The review queue for `supersede_propose`: what is pending, and what
//! became of earlier proposals. Read scope, so a proposer can see its
//! own outcomes as well as a reviewer seeing the queue.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    tools::McpState,
};
use klams_store::Store;
use klams_types::{SupersedeProposal, SupersedeProposalStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

/// Which proposals to list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatusFilter {
    #[default]
    Pending,
    Accepted,
    Rejected,
    Stale,
    All,
}

impl ProposalStatusFilter {
    fn status(self) -> Option<SupersedeProposalStatus> {
        match self {
            Self::Pending => Some(SupersedeProposalStatus::Pending),
            Self::Accepted => Some(SupersedeProposalStatus::Accepted),
            Self::Rejected => Some(SupersedeProposalStatus::Rejected),
            Self::Stale => Some(SupersedeProposalStatus::Stale),
            Self::All => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SupersedeProposalsListArgs {
    /// Which proposals: `pending` (the default), `accepted`, `rejected`,
    /// `stale`, or `all`.
    #[serde(default)]
    pub status: Option<ProposalStatusFilter>,
    /// Maximum rows, oldest first (1..=200, default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupersedeProposalsListOutput {
    pub proposals: Vec<SupersedeProposal>,
}

/// Execute `supersede_proposals_list`.
///
/// # Errors
/// `INVALID_LIMIT`, or the mapped store error.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: SupersedeProposalsListArgs,
) -> Result<SupersedeProposalsListOutput, ErrorEnvelope> {
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(envelope(
            errors::INVALID_LIMIT,
            format!("limit must be 1..={MAX_LIMIT}"),
        ));
    }
    let status = args.status.unwrap_or_default().status();
    let proposals = state
        .store
        .list_supersede_proposals(status, limit)
        .await
        .map_err(|e| errors::from_store_error("list_supersede_proposals", &e))?;
    Ok(SupersedeProposalsListOutput { proposals })
}
