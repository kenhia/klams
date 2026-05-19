//! Source-trust policy table (sprint 003 — US5).
//!
//! Defines the four-way trust hierarchy
//! (`User > Controller > Task > AgentProposal`) as a single
//! `PolicyTable` value. Both the write dispatcher and the
//! `GET /memory/policy` endpoint MUST derive from this same struct;
//! see [`crate::policy::PolicyTable::rank_of`] for the dispatcher
//! consumer.
//!
//! Serialisation shape (matches `contracts/memory_policy.md`):
//!
//! ```json
//! { "User":          { "rank": 4, "description": "..." },
//!   "Controller":    { "rank": 3, "description": "..." },
//!   "Task":          { "rank": 2, "description": "..." },
//!   "AgentProposal": { "rank": 1, "description": "..." } }
//! ```

use klams_types::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyEntry {
    pub rank: u8,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyTable {
    #[serde(rename = "User")]
    pub user: PolicyEntry,
    #[serde(rename = "Controller")]
    pub controller: PolicyEntry,
    #[serde(rename = "Task")]
    pub task: PolicyEntry,
    #[serde(rename = "AgentProposal")]
    pub agent_proposal: PolicyEntry,
}

impl Default for PolicyTable {
    fn default() -> Self {
        Self {
            user: PolicyEntry {
                rank: Source::User.trust_rank(),
                description:
                    "Direct user input via viewport or CLI; wins all contradictions.".into(),
            },
            controller: PolicyEntry {
                rank: Source::Controller.trust_rank(),
                description:
                    "Controller process on a trusted homelab machine; wins over Task and below."
                        .into(),
            },
            task: PolicyEntry {
                rank: Source::Task.trust_rank(),
                description:
                    "Ansible plays, scanner, monitors, controller execution traces.".into(),
            },
            agent_proposal: PolicyEntry {
                rank: Source::AgentProposal.trust_rank(),
                description:
                    "Agent-originated writes; diverted to dissents when they contradict any higher-trust row."
                        .into(),
            },
        }
    }
}

impl PolicyTable {
    /// Rank for a given `Source`. Higher = more trusted. Delegates to
    /// [`Source::trust_rank`] so the dispatcher and the
    /// `GET /memory/policy` endpoint cannot drift (SC-005).
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn rank_of(&self, source: Source) -> u8 {
        source.trust_rank()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_has_strictly_descending_ranks() {
        let t = PolicyTable::default();
        assert!(t.user.rank > t.controller.rank);
        assert!(t.controller.rank > t.task.rank);
        assert!(t.task.rank > t.agent_proposal.rank);
    }

    #[test]
    fn rank_of_matches_default_entries() {
        let t = PolicyTable::default();
        assert_eq!(t.rank_of(Source::User), 4);
        assert_eq!(t.rank_of(Source::Controller), 3);
        assert_eq!(t.rank_of(Source::Task), 2);
        assert_eq!(t.rank_of(Source::AgentProposal), 1);
    }

    #[test]
    fn policy_table_serializes_to_keyed_object() {
        let v = serde_json::to_value(PolicyTable::default()).unwrap();
        let obj = v.as_object().expect("top-level object");
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                &"AgentProposal".to_string(),
                &"Controller".to_string(),
                &"Task".to_string(),
                &"User".to_string(),
            ]
        );
        for source in ["User", "Controller", "Task", "AgentProposal"] {
            let entry = obj.get(source).expect(source).as_object().unwrap();
            assert!(entry.contains_key("rank"), "{source} missing rank");
            assert!(
                entry.contains_key("description"),
                "{source} missing description"
            );
            assert!(entry["rank"].is_number());
            assert!(entry["description"].is_string());
        }
    }
}
