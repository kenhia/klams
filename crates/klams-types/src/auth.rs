//! Identity auth model with per-identity scopes.
//!
//! [`Scope`] enumerates the four permission levels exposed by both the
//! REST surface and the MCP server. [`IdentityConfig`] is the TOML-side
//! shape of an `[[auth.identities]]` row; `klams_api::Identity` is the
//! materialized runtime form.
//!
//! # Sprint 052 — the bearer path is gone
//!
//! Sprint 049 made a declared `X-Homelab-Agent` name the credential and
//! left `[[auth.tokens]]` parsing for the transition window. Sprint 050
//! deleted every row, which is what closed that window (049 D-1: the
//! rows *were* the flag). This sprint deletes the code: there is no
//! `TokenGrantConfig`, no `bearer_token`, and no token table.
//!
//! Because [`AuthConfig`] is not `deny_unknown_fields`, simply removing
//! the fields would make a surviving `[[auth.tokens]]` row **silently
//! ignored** — an operator would believe a credential is live when it
//! authenticates nothing. [`retired_fields`] scans the raw config text
//! before it is parsed and names what it found, so a stale config
//! refuses to start rather than starting wrong (sprint 052 D-1,
//! borrowed from kaed 024 D-1).

use serde::{Deserialize, Serialize};

/// Permission tier attached to an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    /// Sprint 025 (#633) — cross-author curation: manage a memory whose
    /// author is somebody else. Self-management (deleting a memory you
    /// wrote) needs only [`Scope::Write`]; this tier is what a trusted
    /// agent needs to retract *another* author's stale record so the
    /// next session isn't misled by it. Deliberately **not** implied by
    /// [`Scope::Admin`], which keeps its own exclusivity over
    /// hard-delete / restore / list-deleted.
    Manage,
    Admin,
}

impl Scope {
    /// Returns true if a token holding `self` satisfies a route that
    /// requires `needed`. Scopes are independent (not hierarchical) — a
    /// "write" token does not automatically grant "read" unless the
    /// configured grant explicitly lists both. This holds for
    /// [`Scope::Manage`] too: it is a peer of the other three, so a
    /// grant that should curate cross-author must list `manage`
    /// alongside `read`/`write`.
    #[must_use]
    pub fn satisfies(self, needed: Scope) -> bool {
        self == needed
    }

    /// The scope's wire/TOML spelling — the same lowercase token used in
    /// `scopes = [...]` and in `scope_insufficient` error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Manage => "manage",
            Self::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// TOML-facing identity entry (`[[auth.identities]]`, sprint 049).
///
/// The successor to the retired `[[auth.tokens]]` grant: same scope
/// set, same author binding, no secret. The caller declares its name in
/// the `X-Homelab-Agent` header and klams looks the row up by that name.
///
/// Under the homelab threat model — single user, his agents, one
/// tailnet, agents already holding sudo everywhere — a bearer token's
/// only job was to say *which agent*. A declared name does that without
/// a secret, so there is nothing to rotate and nothing to leak.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// The declared name. It is the row's **key**, so it is mandatory:
    /// there is nothing else to look the row up by.
    pub agent_name: String,
    pub scopes: Vec<Scope>,
    #[serde(default)]
    pub label: Option<String>,
    /// Tailnet node short names this identity may arrive from.
    ///
    /// Consulted **only** when `[auth.whois] enforce = true`, which is
    /// off by default (sprint 049 / WI 2389). An identity that declares
    /// no nodes is unconstrained even with enforcement on — pinning is
    /// opt-in per identity, so turning the toggle on cannot lock out
    /// every caller at once.
    #[serde(default)]
    pub nodes: Vec<String>,
}

/// `[auth.whois]` — the tailnet cross-check (sprint 049, WI 2389).
///
/// Record-only by default: the resolved node name is written beside the
/// declared `agent_name` in the request log so "why did X come from Y"
/// is answerable, and **a whois failure never refuses a request**.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhoisConfig {
    /// Resolve the caller's tailnet address at all. Leave on for a
    /// tailnet deployment; turn it off on a host with no `tailscale`
    /// binary so the resolver is never invoked.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Refuse a request whose resolved node is not in the identity's
    /// `nodes` list. **Default off** — this is the mechanism being put
    /// in place, not switched on.
    #[serde(default)]
    pub enforce: bool,
    /// How long a resolved (or unresolvable) address stays cached.
    #[serde(default = "default_whois_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
}

fn default_true() -> bool {
    true
}

fn default_whois_cache_ttl_secs() -> u64 {
    300
}

impl Default for WhoisConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            enforce: false,
            cache_ttl_secs: default_whois_cache_ttl_secs(),
        }
    }
}

/// Validation errors for an identity configuration.
#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    /// Sprint 052: `[[auth.identities]]` is the only table, so this is
    /// simply "no grants at all".
    #[error("auth: at least one `[[auth.identities]]` entry must be set")]
    NoIdentities,
    #[error("auth: identity must declare at least one scope")]
    EmptyScopes,
    #[error("auth: identity `agent_name` is invalid ({reason})")]
    InvalidAgentName { reason: AgentNameInvalidReason },
    /// Sprint 049: `agent_name` is the identities table's key, so two
    /// rows claiming the same one make the lookup ambiguous. Refusing
    /// is the only honest answer — silently picking the first would
    /// attribute writes to whichever row happened to be earlier in the
    /// file.
    #[error("auth: duplicate `agent_name` {agent_name:?} in `[[auth.identities]]`")]
    DuplicateIdentity { agent_name: String },
}

/// Reason an `agent_name` failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentNameInvalidReason {
    /// Empty after trim.
    Empty,
    /// Outside the 2..=64 byte length window.
    Length,
    /// Contains a character outside `[a-z0-9_-]`.
    Charset,
}

impl std::fmt::Display for AgentNameInvalidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("empty"),
            Self::Length => f.write_str("length"),
            Self::Charset => f.write_str("charset"),
        }
    }
}

/// Validate a bearer-token `agent_name` per
/// `sprints/009-stability-attribution/contracts/token-grant-config.md`:
/// non-empty after trim, 2..=64 bytes, charset `[a-z0-9_-]`.
///
/// # Errors
/// Returns [`AgentNameInvalidReason`] describing the first failing
/// rule. Callers wrap this into [`AuthConfigError::InvalidAgentName`].
pub fn validate_agent_name(name: &str) -> Result<(), AgentNameInvalidReason> {
    if name.is_empty() {
        return Err(AgentNameInvalidReason::Empty);
    }
    let len = name.len();
    if !(2..=64).contains(&len) {
        return Err(AgentNameInvalidReason::Length);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(AgentNameInvalidReason::Charset);
    }
    Ok(())
}

impl IdentityConfig {
    /// Apply per-identity validation: `agent_name` present and legal,
    /// and a non-empty scope set.
    ///
    /// An identity row cannot be unattributable — the name *is* the
    /// credential — so sprint 034's "a privileged grant must declare an
    /// `agent_name`" rule has nothing to check here. It retired with
    /// the token table in sprint 052.
    ///
    /// # Errors
    /// [`AuthConfigError::InvalidAgentName`] if `agent_name` fails the
    /// charset/length rules, or [`AuthConfigError::EmptyScopes`] if
    /// `scopes` is empty.
    pub fn validate(&self) -> Result<(), AuthConfigError> {
        if let Err(reason) = validate_agent_name(&self.agent_name) {
            return Err(AuthConfigError::InvalidAgentName { reason });
        }
        if self.scopes.is_empty() {
            return Err(AuthConfigError::EmptyScopes);
        }
        Ok(())
    }
}

/// The `[auth]` block of `klams.toml`.
///
/// Sprint 045 (#265): this lived in `klams-service::config` until
/// `klams-token` needed it. That CLI edits the very grants this struct
/// describes, and a config editor whose idea of the schema can drift
/// from the service's is the class of bug it exists to prevent — so the
/// shape moved down here, where both can share exactly one definition.
/// `klams_service::config::AuthConfig` is now a re-export of this type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Declared identities (`[[auth.identities]]`, sprint 049). Keyed
    /// on `agent_name`, presented by the caller in `X-Homelab-Agent`.
    #[serde(default)]
    pub identities: Vec<IdentityConfig>,

    /// `[auth.whois]` — the tailnet cross-check (WI 2389). Absent block
    /// means resolve-and-record with enforcement off.
    #[serde(default)]
    pub whois: WhoisConfig,
}

impl AuthConfig {
    /// Every reason this `[auth]` block would be refused, as operator-
    /// facing strings — *all* of them, not just the first, because an
    /// operator fixing a config wants the whole list in one pass.
    ///
    /// This is the single definition of "is this `[auth]` block
    /// startable": `klams-service --validate-config` reports it, and
    /// `klams-token` gates every write on it (sprint 045, #265). An
    /// empty vec means the block would boot.
    #[must_use]
    pub fn errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.identities.is_empty() {
            errors.push(format!("[auth]: {}", AuthConfigError::NoIdentities));
        }
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (i, id) in self.identities.iter().enumerate() {
            if let Err(e) = id.validate() {
                errors.push(format!(
                    "[auth.identities[{i}]] ({label}): {e}",
                    label = id.label.as_deref().unwrap_or(&id.agent_name)
                ));
            }
            if !seen.insert(id.agent_name.as_str()) {
                errors.push(format!(
                    "[auth.identities[{i}]]: {}",
                    AuthConfigError::DuplicateIdentity {
                        agent_name: id.agent_name.clone()
                    }
                ));
            }
        }
        errors
    }

    /// Non-fatal observations about this `[auth]` block.
    ///
    /// An identity needs no `label` warning: unlike a token grant, its
    /// `agent_name` is already the attribution, so there is never an
    /// empty one to warn about.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();
        // Sprint 049: enforcement is per-identity opt-in, so turning the
        // toggle on with no `nodes` anywhere enforces nothing. That is a
        // config that looks locked down and is not — worth saying out
        // loud rather than discovering from an audit.
        if self.whois.enforce {
            let unpinned: Vec<&str> = self
                .identities
                .iter()
                .filter(|i| i.nodes.is_empty())
                .map(|i| i.agent_name.as_str())
                .collect();
            if !unpinned.is_empty() {
                let (noun, declares, is) = if unpinned.len() == 1 {
                    ("identity", "declares", "is")
                } else {
                    ("identities", "declare", "are")
                };
                warnings.push(format!(
                    "[auth.whois]: enforce = true, but {n} {noun} {declares} no `nodes` and \
                     {is} therefore unconstrained: {list}",
                    n = unpinned.len(),
                    list = unpinned.join(", ")
                ));
            }
        }
        warnings
    }
}

/// Scope set of the caller, resolved from the presented bearer token.
///
/// Stamped onto request extensions by the REST auth middleware and read
/// back by both surfaces — REST route guards directly, MCP tools via the
/// `http::request::Parts` that rmcp copies into the tool context.
///
/// Sprint 031 (#645): this and [`AuthenticatedAuthor`] used to live in
/// `klams-api`, which made `klams-mcp` depend on the REST crate for two
/// plain data types and nothing else — an inverted dependency that also
/// meant the MCP server could not be built or tested without the REST
/// layer. They are pure data, so they belong at the bottom with
/// [`Scope`].
#[derive(Clone, Debug)]
pub struct AuthenticatedScopes(pub std::sync::Arc<Vec<Scope>>);

/// Author bound to the request's bearer token (sprint 009). Write paths
/// stamp `author_id` from this when the caller omits one.
#[derive(Clone, Debug)]
pub struct AuthenticatedAuthor {
    pub author_id: uuid::Uuid,
    pub agent_name: std::sync::Arc<String>,
}

/// The caller's tailnet origin, as resolved by `tailscale whois`
/// (sprint 049, WI 2389). Stamped on every authenticated request.
///
/// `node` is `None` when whois is disabled, the address is absent, or
/// tailscaled did not answer — all three are reported as `unknown`, and
/// none of them refuses the request.
#[derive(Clone, Debug)]
pub struct AuthenticatedPeer {
    /// The address whois was asked about, when there was one.
    pub addr: Option<std::net::IpAddr>,
    /// Short tailnet node name, e.g. `kai`.
    pub node: Option<String>,
}

impl AuthenticatedPeer {
    /// The node name for logs: the resolved short name, or the literal
    /// `unknown` (WI 2389 — "`unknown` when tailscaled does not
    /// answer").
    #[must_use]
    pub fn node_or_unknown(&self) -> &str {
        self.node.as_deref().unwrap_or("unknown")
    }
}

/// A config key or table this build no longer honours, with everything
/// an operator needs to fix it in one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetiredField {
    /// The spelling as it appears in the file, e.g. `[[auth.tokens]]`.
    pub spelling: &'static str,
    /// The sprint that retired it.
    pub sprint: &'static str,
    /// What to do instead.
    pub fix: &'static str,
}

impl std::fmt::Display for RetiredField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "config names `{}`, retired in sprint {}: {}",
            self.spelling, self.sprint, self.fix
        )
    }
}

/// Every retired spelling, longest first. Order is load-bearing — see
/// [`retired_fields`].
const RETIRED: &[RetiredField] = &[
    RetiredField {
        spelling: "[[auth.tokens]]",
        sprint: "052",
        fix: "delete the row and add an `[[auth.identities]]` row with the same \
              `agent_name` and `scopes` (`sudo klams-token identity add`); callers \
              send `X-Homelab-Agent: <agent_name>` instead of `Authorization: Bearer`",
    },
    RetiredField {
        spelling: "bearer_token",
        sprint: "034",
        fix: "delete the line and add an `[[auth.identities]]` row \
              (`sudo klams-token identity add`)",
    },
];

/// Strip TOML comments, respecting quoted strings.
///
/// Load-bearing for [`retired_fields`]: this repo's own
/// `klams.example.toml` documents the retired forms in prose, and a
/// guard that refused to start over a comment would fail exactly the
/// operators it exists to protect. Quote-awareness matters in the other
/// direction — a `#` inside a Postgres URL is not a comment, and
/// truncating there would hide a retired key that followed it on the
/// same line.
#[must_use]
pub fn strip_toml_comments(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let mut quote: Option<char> = None;
        let mut end = line.len();
        for (i, c) in line.char_indices() {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '#') => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

/// Scan raw config text for spellings this build no longer honours.
///
/// [`AuthConfig`] is not `deny_unknown_fields`, so a surviving
/// `[[auth.tokens]]` row would otherwise be *silently ignored* — the
/// operator believes a credential is live and it authenticates nothing.
/// Refusing to start, naming the field and the fix, is the honest
/// outcome (sprint 052 D-1).
///
/// Two properties are load-bearing, both learned from kaed 024 D-1:
///
/// 1. **Comments are stripped first** ([`strip_toml_comments`]), so
///    prose *about* the cutover — which the shipped example config
///    carries — is not a refusal.
/// 2. **Longest match first, and the match is consumed.** `bearer_token`
///    is a substring of nothing here today, but `[[auth.tokens]]`
///    contains `tokens`, and the naive scan reports fields the file
///    never named. Consuming each match makes the report describe the
///    file rather than the pattern list.
#[must_use]
pub fn retired_fields(raw: &str) -> Vec<RetiredField> {
    let mut haystack = strip_toml_comments(raw);
    let mut found = Vec::new();
    // RETIRED is ordered longest-spelling first; consume every
    // occurrence of each before considering a shorter one.
    for spec in RETIRED {
        if haystack.contains(spec.spelling) {
            found.push(*spec);
            haystack = haystack.replace(spec.spelling, "");
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_satisfies_is_exact() {
        assert!(Scope::Read.satisfies(Scope::Read));
        assert!(!Scope::Write.satisfies(Scope::Read));
        assert!(!Scope::Admin.satisfies(Scope::Write));
    }

    /// Sprint 025 (#633) — `manage` is a peer of the other three, not a
    /// super-scope. Granting `admin` must NOT confer cross-author
    /// curation, and holding `manage` must not confer hard-delete.
    #[test]
    fn manage_scope_is_flat_like_the_others() {
        assert!(Scope::Manage.satisfies(Scope::Manage));
        assert!(!Scope::Admin.satisfies(Scope::Manage));
        assert!(!Scope::Manage.satisfies(Scope::Admin));
        assert!(!Scope::Write.satisfies(Scope::Manage));
        assert!(!Scope::Manage.satisfies(Scope::Write));
    }

    #[test]
    fn manage_scope_round_trips_through_toml_lowercase() {
        let id: IdentityConfig = toml::from_str(
            r#"
            scopes     = ["read", "write", "manage"]
            label      = "claude"
            agent_name = "claude"
            "#,
        )
        .expect("manage must parse as a scope in [[auth.identities]]");
        assert_eq!(id.scopes, vec![Scope::Read, Scope::Write, Scope::Manage]);
        id.validate().unwrap();
    }

    // -----------------------------------------------------------------
    // Sprint 049 — `[[auth.identities]]`.
    // -----------------------------------------------------------------

    #[test]
    fn identity_round_trips_through_toml() {
        let id: IdentityConfig = toml::from_str(
            r#"
            agent_name = "claude"
            scopes     = ["read", "write", "manage"]
            label      = "claude"
            nodes      = ["kai", "kubs0"]
            "#,
        )
        .expect("[[auth.identities]] must parse");
        assert_eq!(id.agent_name, "claude");
        assert_eq!(id.scopes, vec![Scope::Read, Scope::Write, Scope::Manage]);
        assert_eq!(id.nodes, vec!["kai".to_string(), "kubs0".to_string()]);
        id.validate().unwrap();
    }

    /// `nodes` is opt-in: the common row omits it, and omitting it must
    /// not make the row invalid.
    #[test]
    fn identity_without_nodes_is_valid() {
        let id: IdentityConfig = toml::from_str(
            r#"
            agent_name = "klams-view"
            scopes     = ["read"]
            "#,
        )
        .expect("an identity without `nodes` or `label` must parse");
        assert!(id.nodes.is_empty());
        assert!(id.label.is_none());
        id.validate().unwrap();
    }

    /// The identities table has no unattributable row by construction —
    /// sprint 034's `PrivilegedGrantNeedsAgentName` cannot arise here,
    /// because the name *is* the credential. A privileged identity
    /// validates with nothing extra.
    #[test]
    fn privileged_identity_needs_no_extra_attribution() {
        let id = IdentityConfig {
            agent_name: "ken_admin".into(),
            scopes: vec![Scope::Read, Scope::Write, Scope::Manage, Scope::Admin],
            label: Some("ken-admin".into()),
            nodes: vec![],
        };
        id.validate().unwrap();
    }

    #[test]
    fn identity_rejects_bad_name_and_empty_scopes() {
        let bad_name = IdentityConfig {
            agent_name: "Claude".into(), // uppercase is outside the charset
            scopes: vec![Scope::Read],
            label: None,
            nodes: vec![],
        };
        assert!(matches!(
            bad_name.validate(),
            Err(AuthConfigError::InvalidAgentName {
                reason: AgentNameInvalidReason::Charset
            })
        ));

        let no_scopes = IdentityConfig {
            agent_name: "claude".into(),
            scopes: vec![],
            label: None,
            nodes: vec![],
        };
        assert!(matches!(
            no_scopes.validate(),
            Err(AuthConfigError::EmptyScopes)
        ));
    }

    /// Sprint 052: there is one table now. Identities boot; nothing
    /// does not.
    #[test]
    fn identities_are_the_only_grant_table() {
        let identities_only = AuthConfig {
            identities: vec![IdentityConfig {
                agent_name: "claude".into(),
                scopes: vec![Scope::Read, Scope::Write],
                label: None,
                nodes: vec![],
            }],
            ..AuthConfig::default()
        };
        assert!(
            identities_only.errors().is_empty(),
            "identities alone must boot: {:?}",
            identities_only.errors()
        );

        let neither = AuthConfig::default();
        assert_eq!(neither.errors().len(), 1);
        assert!(neither.errors()[0].contains("auth.identities"));
    }

    /// `agent_name` is the identities table's key, so a duplicate is
    /// refused rather than resolved by file order.
    #[test]
    fn duplicate_identity_agent_name_is_rejected() {
        let cfg = AuthConfig {
            identities: vec![
                IdentityConfig {
                    agent_name: "claude".into(),
                    scopes: vec![Scope::Read],
                    label: Some("first".into()),
                    nodes: vec![],
                },
                IdentityConfig {
                    agent_name: "claude".into(),
                    scopes: vec![Scope::Read, Scope::Write],
                    label: Some("second".into()),
                    nodes: vec![],
                },
            ],
            ..AuthConfig::default()
        };
        let errors = cfg.errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("duplicate"), "{errors:?}");
        assert!(errors[0].contains("claude"), "{errors:?}");
    }

    /// The whois block defaults to resolve-and-record: enforcement is
    /// the mechanism being put in place, not switched on (WI 2389).
    #[test]
    fn whois_defaults_to_recording_without_enforcing() {
        let w = WhoisConfig::default();
        assert!(w.enabled);
        assert!(!w.enforce, "enforcement must default OFF");
        assert_eq!(w.cache_ttl_secs, 300);

        // And an `[auth]` block with no `[auth.whois]` at all gets them.
        let cfg: AuthConfig = toml::from_str(
            r#"
            [[identities]]
            agent_name = "claude"
            scopes     = ["read"]
            "#,
        )
        .expect("parse");
        assert!(cfg.whois.enabled);
        assert!(!cfg.whois.enforce);
    }

    /// Enforcement is per-identity opt-in, so `enforce = true` with no
    /// `nodes` anywhere enforces nothing. That config looks locked down
    /// and is not, so it must say so.
    #[test]
    fn enforce_without_any_pinned_nodes_warns() {
        let cfg = AuthConfig {
            identities: vec![
                IdentityConfig {
                    agent_name: "claude".into(),
                    scopes: vec![Scope::Read],
                    label: None,
                    nodes: vec![],
                },
                IdentityConfig {
                    agent_name: "kmon".into(),
                    scopes: vec![Scope::Read],
                    label: None,
                    nodes: vec!["kubs0".into()],
                },
            ],
            whois: WhoisConfig {
                enforce: true,
                ..WhoisConfig::default()
            },
        };
        assert!(cfg.errors().is_empty());
        let warnings = cfg.warnings();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("claude"), "{warnings:?}");
        // The operator reads this line; it should be a sentence.
        assert!(
            warnings[0].contains("1 identity declares no `nodes` and is therefore"),
            "{warnings:?}"
        );
        assert!(
            !warnings[0].contains("kmon"),
            "a pinned identity is constrained and must not be listed: {warnings:?}"
        );

        // Enforcement off — nothing to warn about, pinned or not.
        let off = AuthConfig {
            whois: WhoisConfig::default(),
            ..cfg
        };
        assert!(off.warnings().is_empty(), "{:?}", off.warnings());
    }

    #[test]
    fn authenticated_peer_reports_unknown_when_unresolved() {
        let peer = AuthenticatedPeer {
            addr: None,
            node: None,
        };
        assert_eq!(peer.node_or_unknown(), "unknown");

        let resolved = AuthenticatedPeer {
            addr: Some("100.97.109.60".parse().unwrap()),
            node: Some("kai".into()),
        };
        assert_eq!(resolved.node_or_unknown(), "kai");
    }

    #[test]
    fn agent_name_accepts_valid_shapes() {
        for ok in [
            "alice",
            "klams-bench",
            "agent_42",
            "ab",
            "a-b-c-d",
            "ansible-k",
        ] {
            assert!(validate_agent_name(ok).is_ok(), "expected {ok} to validate");
        }
    }

    #[test]
    fn agent_name_rejects_empty() {
        assert_eq!(validate_agent_name(""), Err(AgentNameInvalidReason::Empty));
    }

    #[test]
    fn agent_name_rejects_charset() {
        for bad in ["Alice", "alice!", "alice space", "Aa"] {
            assert_eq!(
                validate_agent_name(bad),
                Err(AgentNameInvalidReason::Charset),
                "expected {bad} to be rejected for charset"
            );
        }
    }

    #[test]
    fn agent_name_rejects_length() {
        assert_eq!(
            validate_agent_name("a"),
            Err(AgentNameInvalidReason::Length)
        );
        let long = "a".repeat(65);
        assert_eq!(
            validate_agent_name(&long),
            Err(AgentNameInvalidReason::Length)
        );
    }

    #[test]
    fn identity_rejects_invalid_agent_name() {
        let g = IdentityConfig {
            agent_name: "Alice".into(),
            scopes: vec![Scope::Read],
            label: None,
            nodes: vec![],
        };
        let err = g.validate().unwrap_err();
        match err {
            AuthConfigError::InvalidAgentName { reason } => {
                assert_eq!(reason, AgentNameInvalidReason::Charset);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Sprint 052 — the retired-field guard (D-1).
    // -----------------------------------------------------------------

    /// The whole point: a config that still carries a token row must
    /// refuse to start, naming the field and the fix. Without this the
    /// row is silently ignored (`AuthConfig` is not
    /// `deny_unknown_fields`) and the operator believes a credential is
    /// live when it authenticates nothing.
    #[test]
    fn retired_token_table_is_reported() {
        let raw = r#"
[auth]
[[auth.tokens]]
token      = "abcdefghijklmnop"
agent_name = "claude"
scopes     = ["read", "write"]
"#;
        let found = retired_fields(raw);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].spelling, "[[auth.tokens]]");
        let msg = found[0].to_string();
        assert!(msg.contains("[[auth.tokens]]"), "{msg}");
        assert!(msg.contains("052"), "{msg}");
        assert!(msg.contains("auth.identities"), "{msg}");
    }

    /// Sprint 034's form is retired too, and says so under its own
    /// sprint number rather than this one's.
    #[test]
    fn retired_bearer_token_key_is_reported_under_sprint_034() {
        let found = retired_fields("[auth]\nbearer_token = \"abcdefghijklmnop\"\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].spelling, "bearer_token");
        assert_eq!(found[0].sprint, "034");
    }

    /// The property that makes the guard usable rather than a
    /// fleet-wide outage: this repo ships an example config documenting
    /// the retired forms in prose. A guard that refused to start over a
    /// comment would fail exactly the operators it protects (kaed 024
    /// D-1).
    #[test]
    fn prose_about_the_cutover_is_not_a_refusal() {
        let raw = r#"
# [[auth.tokens]] — RETIRED (sprint 052). Do not add rows here.
# The old `bearer_token` form went in sprint 034.
[auth]
[[auth.identities]]
agent_name = "claude"
scopes     = ["read", "write"]
"#;
        assert!(retired_fields(raw).is_empty(), "{:?}", retired_fields(raw));
    }

    /// A `#` inside a quoted value is not a comment. Truncating there
    /// would hide a retired key that followed it on the same line —
    /// the guard failing open, which is the one way it must not fail.
    #[test]
    fn a_hash_inside_a_quoted_value_does_not_start_a_comment() {
        let raw = "[postgres]\nurl = \"postgres://u:p#ass@localhost/db\" # real comment\n";
        assert_eq!(
            strip_toml_comments(raw).trim_end(),
            "[postgres]\nurl = \"postgres://u:p#ass@localhost/db\""
        );
        // And the guard still sees a retired key sharing that line.
        let sneaky = "url = \"p#ass\"\nbearer_token = \"x\"\n";
        assert_eq!(retired_fields(sneaky).len(), 1);
    }

    /// Longest-first, and the match is consumed: `[[auth.tokens]]` must
    /// report once, as itself, not also as some shorter pattern it
    /// contains. Pinned because the naive scan reports fields the file
    /// never named (kaed 024 D-1's second property).
    #[test]
    fn retired_spellings_are_ordered_longest_first() {
        let lengths: Vec<usize> = RETIRED.iter().map(|r| r.spelling.len()).collect();
        let mut sorted = lengths.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(lengths, sorted, "RETIRED must stay ordered longest-first");
    }

    /// Both forms in one file are both reported — an operator fixing a
    /// config wants the whole list in one pass, the same rule
    /// [`AuthConfig::errors`] follows.
    #[test]
    fn every_retired_form_present_is_reported() {
        let raw = "bearer_token = \"x\"\n[[auth.tokens]]\ntoken = \"y\"\n";
        let found = retired_fields(raw);
        assert_eq!(found.len(), 2, "{found:?}");
    }

    /// A clean identities-only config — what sprint 050 left on kubs0 —
    /// passes the guard. This is the live precondition for the deploy.
    #[test]
    fn an_identities_only_config_passes_the_guard() {
        let raw = r#"
[auth]
[[auth.identities]]
agent_name = "claude"
scopes     = ["read", "write", "manage"]
"#;
        assert!(retired_fields(raw).is_empty());
    }
}
