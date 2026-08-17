//! Fingerprint-and-refuse: the guard that makes a clobber impossible.
//!
//! k-homelab sprint 016 (S4) needed to remove one grant from a file
//! holding thirteen, and rather than trust the edit it fingerprinted
//! every grant as `{identity: sha256(token)[:12]}` before and after and
//! refused to write unless the after-set was exactly the before-set
//! minus the target. It caught nothing, because it had nothing to catch
//! — but it is the shape of the guarantee, and S4's recommendation was
//! that whoever built this tool lift it. This module is that lift,
//! generalized from "remove" to every mutation the CLI performs.
//!
//! Fingerprints deliberately cover **identity and token value only**,
//! not scopes. That is what makes [`Change::None`] meaningful: a scopes
//! edit must leave the entire grant *set* byte-identical, so the guard
//! catches a scopes edit that damaged a token or dropped a grant, which
//! is the failure that actually hurts.

use sha2::{Digest, Sha256};

/// One grant reduced to what a write must not accidentally change.
///
/// `key` is the grant's identity as klams itself keys it: `agent_name`
/// when present. That matters beyond bookkeeping — klams attributes
/// memories by `agent_name`, not by token value, so a rotation that
/// preserved the token but moved the identity would orphan everything
/// that agent ever wrote.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GrantFingerprint {
    pub key: String,
    pub token: String,
}

impl GrantFingerprint {
    #[must_use]
    pub fn new(key: impl Into<String>, token_value: &str) -> Self {
        Self {
            key: key.into(),
            token: token_digest(token_value),
        }
    }
}

impl std::fmt::Display for GrantFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}={}", self.key, self.token)
    }
}

/// `sha256(token)[:12]` — enough to prove two token values differ
/// without ever putting one in a terminal, a log or a diff.
#[must_use]
pub fn token_digest(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest
        .iter()
        .take(6)
        .fold(String::with_capacity(12), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

/// The one change a write declares it is making. Anything the delta
/// shows beyond this aborts the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// `scopes` — the grant *set* must come out identical.
    None,
    /// `add` — exactly one new fingerprint, nothing else moved.
    Added(GrantFingerprint),
    /// `remove` — exactly one fingerprint gone, nothing else moved.
    Removed(GrantFingerprint),
    /// `rotate` — this identity's token digest changes and nothing
    /// else does. The identity itself must survive: see
    /// [`GrantFingerprint`].
    Rotated { key: String },
}

/// Why a write was refused. Every variant is a bug the operator wants
/// to hear about loudly, not a condition to recover from.
#[derive(Debug, thiserror::Error)]
pub enum DeltaError {
    #[error(
        "refusing to write: {intent} should have left every other grant untouched, but \
         {gained} grant(s) appeared and {lost} disappeared\n  gained: {gained_list}\n  lost: {lost_list}"
    )]
    Unexpected {
        intent: &'static str,
        gained: usize,
        lost: usize,
        gained_list: String,
        lost_list: String,
    },
    #[error(
        "refusing to write: rotating `{key}` must change that grant's token and nothing else, \
         but the delta was\n  gained: {gained_list}\n  lost: {lost_list}"
    )]
    BadRotation {
        key: String,
        gained_list: String,
        lost_list: String,
    },
    #[error("refusing to write: rotating `{key}` produced the same token value")]
    RotationNoop { key: String },
}

/// Compare the before/after fingerprint sets against the declared
/// change, and refuse anything else.
///
/// # Errors
/// [`DeltaError`] describing exactly what moved that should not have.
pub fn verify_delta(
    before: &[GrantFingerprint],
    after: &[GrantFingerprint],
    change: &Change,
) -> Result<(), DeltaError> {
    let gained = difference(after, before);
    let lost = difference(before, after);

    match change {
        Change::None => expect(&gained, &lost, &[], &[], "this edit"),
        Change::Added(fp) => expect(&gained, &lost, std::slice::from_ref(fp), &[], "add"),
        Change::Removed(fp) => expect(&gained, &lost, &[], std::slice::from_ref(fp), "remove"),
        Change::Rotated { key } => {
            // An empty delta means the new token hashed to the old one
            // — the fingerprints cancelled out. Say that, rather than
            // reporting a rotation failure with nothing to show.
            if gained.is_empty() && lost.is_empty() {
                return Err(DeltaError::RotationNoop { key: key.clone() });
            }
            // Exactly one gained and one lost, both carrying this
            // identity, with a token digest that actually moved.
            let (Some(g), Some(l), 1, 1) = (gained.first(), lost.first(), gained.len(), lost.len())
            else {
                return Err(DeltaError::BadRotation {
                    key: key.clone(),
                    gained_list: render(&gained),
                    lost_list: render(&lost),
                });
            };
            if g.key != *key || l.key != *key {
                return Err(DeltaError::BadRotation {
                    key: key.clone(),
                    gained_list: render(&gained),
                    lost_list: render(&lost),
                });
            }
            if g.token == l.token {
                return Err(DeltaError::RotationNoop { key: key.clone() });
            }
            Ok(())
        }
    }
}

fn expect(
    gained: &[GrantFingerprint],
    lost: &[GrantFingerprint],
    want_gained: &[GrantFingerprint],
    want_lost: &[GrantFingerprint],
    intent: &'static str,
) -> Result<(), DeltaError> {
    if gained == want_gained && lost == want_lost {
        return Ok(());
    }
    Err(DeltaError::Unexpected {
        intent,
        gained: gained.len(),
        lost: lost.len(),
        gained_list: render(gained),
        lost_list: render(lost),
    })
}

/// Multiset difference `a \ b`, sorted — two grants may legitimately
/// share an identity string, and a duplicate silently cancelling out
/// would be the guard lying.
fn difference(a: &[GrantFingerprint], b: &[GrantFingerprint]) -> Vec<GrantFingerprint> {
    let mut remaining: Vec<&GrantFingerprint> = b.iter().collect();
    let mut out = Vec::new();
    for item in a {
        if let Some(pos) = remaining.iter().position(|r| *r == item) {
            remaining.swap_remove(pos);
        } else {
            out.push(item.clone());
        }
    }
    out.sort();
    out
}

fn render(fps: &[GrantFingerprint]) -> String {
    if fps.is_empty() {
        return "(none)".into();
    }
    fps.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(key: &str, token: &str) -> GrantFingerprint {
        GrantFingerprint::new(key, token)
    }

    #[test]
    fn digest_is_twelve_hex_chars_and_hides_the_value() {
        let d = token_digest("alice-secret-token-value");
        assert_eq!(d.len(), 12);
        assert!(d.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!"alice-secret-token-value".contains(&d));
    }

    #[test]
    fn scopes_edit_must_not_move_any_grant() {
        let before = vec![fp("alice", "a"), fp("bench", "b")];
        verify_delta(&before, &before.clone(), &Change::None).unwrap();
    }

    /// The #264 incident, in miniature: an edit that was supposed to
    /// touch only scopes silently rewrote a sibling's token.
    #[test]
    fn scopes_edit_that_clobbers_a_sibling_is_refused() {
        let before = vec![fp("alice", "a"), fp("bench", "b")];
        let after = vec![fp("alice", "a"), fp("bench", "CLOBBERED")];
        let err = verify_delta(&before, &after, &Change::None).unwrap_err();
        assert!(matches!(
            err,
            DeltaError::Unexpected {
                gained: 1,
                lost: 1,
                ..
            }
        ));
    }

    #[test]
    fn add_accepts_exactly_the_new_grant() {
        let before = vec![fp("alice", "a")];
        let after = vec![fp("alice", "a"), fp("mind", "m")];
        verify_delta(&before, &after, &Change::Added(fp("mind", "m"))).unwrap();
    }

    #[test]
    fn add_that_also_dropped_a_grant_is_refused() {
        let before = vec![fp("alice", "a"), fp("bench", "b")];
        let after = vec![fp("alice", "a"), fp("mind", "m")];
        let err = verify_delta(&before, &after, &Change::Added(fp("mind", "m"))).unwrap_err();
        assert!(matches!(err, DeltaError::Unexpected { lost: 1, .. }));
    }

    #[test]
    fn remove_accepts_exactly_the_target() {
        let before = vec![fp("alice", "a"), fp("ansible-k", "dead")];
        let after = vec![fp("alice", "a")];
        verify_delta(&before, &after, &Change::Removed(fp("ansible-k", "dead"))).unwrap();
    }

    #[test]
    fn remove_that_took_a_neighbour_with_it_is_refused() {
        let before = vec![fp("alice", "a"), fp("ansible-k", "dead"), fp("bench", "b")];
        let after = vec![fp("alice", "a")];
        let err =
            verify_delta(&before, &after, &Change::Removed(fp("ansible-k", "dead"))).unwrap_err();
        assert!(
            matches!(err, DeltaError::Unexpected { lost: 2, .. }),
            "{err}"
        );
    }

    #[test]
    fn rotation_changes_one_token_and_keeps_the_identity() {
        let before = vec![fp("alice", "old"), fp("bench", "b")];
        let after = vec![fp("alice", "new"), fp("bench", "b")];
        verify_delta(
            &before,
            &after,
            &Change::Rotated {
                key: "alice".into(),
            },
        )
        .unwrap();
    }

    /// P0.1's finding, as a guard: klams keys identity on `agent_name`,
    /// so a "rotation" that renamed the agent would orphan every memory
    /// that agent wrote. The delta shows it, and the write is refused.
    #[test]
    fn rotation_that_moves_the_identity_is_refused() {
        let before = vec![fp("alice", "old")];
        let after = vec![fp("alice-2", "new")];
        let err = verify_delta(
            &before,
            &after,
            &Change::Rotated {
                key: "alice".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, DeltaError::BadRotation { .. }));
    }

    #[test]
    fn rotation_that_produced_the_same_token_is_refused() {
        let before = vec![fp("alice", "same")];
        let err = verify_delta(
            &before,
            &before.clone(),
            &Change::Rotated {
                key: "alice".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, DeltaError::RotationNoop { .. }), "{err}");
    }

    /// Two grants sharing an identity string must not cancel out in the
    /// diff — a set-based difference would report "nothing moved" when
    /// one of the pair was destroyed.
    #[test]
    fn duplicate_identities_are_compared_as_a_multiset() {
        let before = vec![fp("dup", "one"), fp("dup", "one")];
        let after = vec![fp("dup", "one")];
        let err = verify_delta(&before, &after, &Change::None).unwrap_err();
        assert!(matches!(err, DeltaError::Unexpected { lost: 1, .. }));
    }
}
