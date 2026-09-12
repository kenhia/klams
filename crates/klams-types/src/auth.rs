//! Bearer-token auth model with per-token scopes (sprint 007).
//!
//! [`Scope`] enumerates the three permission levels exposed by both the
//! legacy REST surface and the new MCP server. [`TokenGrantConfig`] is the
//! TOML-side shape (see `data-model.md` §5); [`TokenGrant`] is the
//! materialized runtime form with the token bytes wrapped for constant-time
//! comparison upstream.

use serde::{Deserialize, Serialize};

/// Permission tier attached to a bearer token.
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

/// TOML-facing token grant entry (`[[auth.tokens]]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenGrantConfig {
    pub token: String,
    pub scopes: Vec<Scope>,
    #[serde(default)]
    pub label: Option<String>,
    /// Sprint 009: agent identity bound to this token. Resolved to
    /// an `Author` at service startup; every REST write
    /// authenticated by this token is attributed to that author
    /// instead of `system`. `None` falls back to the seeded
    /// `system` author (back-compat for tokens issued before
    /// sprint 009).
    #[serde(default)]
    pub agent_name: Option<String>,
}

/// TOML-facing identity entry (`[[auth.identities]]`, sprint 049).
///
/// The successor to [`TokenGrantConfig`]: same scope set, same author
/// binding, no secret. The caller declares its name in the
/// `X-Homelab-Agent` header and klams looks the row up by that name.
///
/// Under the homelab threat model — single user, his agents, one
/// tailnet, agents already holding sudo everywhere — a bearer token's
/// only job was to say *which agent*. A declared name does that without
/// a secret, so there is nothing to rotate and nothing to leak.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// The declared name. Unlike [`TokenGrantConfig::agent_name`] this
    /// is the row's **key**, so it is mandatory rather than optional —
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

/// Validation errors for a bearer-token configuration.
#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    /// Sprint 049: an `[[auth.identities]]` row satisfies this too —
    /// the transition window means either table may carry the grants.
    #[error("auth: at least one `[[auth.identities]]` or `[[auth.tokens]]` entry must be set")]
    NoTokens,
    #[error("auth: token must be at least 16 characters")]
    TokenTooShort,
    #[error("auth: token grant must declare at least one scope")]
    EmptyScopes,
    #[error("auth: token grant `agent_name` is invalid ({reason})")]
    InvalidAgentName { reason: AgentNameInvalidReason },
    /// Sprint 034 (#703): every privileged action must be attributable
    /// — the property sprint 025 was built around, closed here.
    #[error(
        "auth: a grant holding `manage` or `admin` must declare `agent_name` \
         so privileged actions are attributable"
    )]
    PrivilegedGrantNeedsAgentName,
    /// Sprint 034 (#703): the legacy single-token form is retired — it
    /// materialized a full-scope grant that could not declare an
    /// `agent_name`, which the rule above now forbids.
    #[error(
        "auth: `bearer_token` is retired (sprint 034); replace it with a \
         `[[auth.tokens]]` grant carrying `agent_name` — see docs/auth.md \
         for the migration note"
    )]
    LegacyBearerTokenRetired,
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

impl TokenGrantConfig {
    /// Apply per-grant validation (length + non-empty scope set,
    /// `agent_name` charset/length when present, and — sprint 034
    /// #703 — `agent_name` *required* on grants holding `manage` or
    /// `admin`, so every privileged action is attributable).
    ///
    /// # Errors
    /// Returns [`AuthConfigError::TokenTooShort`] if the token is under
    /// 16 characters, [`AuthConfigError::EmptyScopes`] if `scopes` is
    /// empty, [`AuthConfigError::InvalidAgentName`] if a non-None
    /// `agent_name` fails the rules in
    /// `sprints/009-stability-attribution/contracts/token-grant-config.md`,
    /// or [`AuthConfigError::PrivilegedGrantNeedsAgentName`] if a
    /// `manage`/`admin` grant declares none.
    pub fn validate(&self) -> Result<(), AuthConfigError> {
        if self.token.len() < 16 {
            return Err(AuthConfigError::TokenTooShort);
        }
        if self.scopes.is_empty() {
            return Err(AuthConfigError::EmptyScopes);
        }
        match &self.agent_name {
            Some(name) => {
                if let Err(reason) = validate_agent_name(name) {
                    return Err(AuthConfigError::InvalidAgentName { reason });
                }
            }
            None => {
                if self
                    .scopes
                    .iter()
                    .any(|s| matches!(s, Scope::Manage | Scope::Admin))
                {
                    return Err(AuthConfigError::PrivilegedGrantNeedsAgentName);
                }
            }
        }
        Ok(())
    }
}

impl IdentityConfig {
    /// Apply per-identity validation: `agent_name` present and legal,
    /// and a non-empty scope set.
    ///
    /// There is no [`AuthConfigError::PrivilegedGrantNeedsAgentName`]
    /// case here — sprint 034 added that rule so every privileged
    /// action would be attributable, and an identity row cannot be
    /// unattributable: the name *is* the credential.
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
    /// RETIRED legacy single-token form (sprint 034, #703). It
    /// materialized a full-scope grant that could not declare an
    /// `agent_name`, which privileged grants now require. The field
    /// still parses — deliberately: a config that carries one refuses
    /// to start with the migration note instead of silently ignoring a
    /// credential the operator believes is live.
    #[serde(default)]
    pub bearer_token: String,

    /// Token grants (`[[auth.tokens]]`). Each entry carries its own
    /// scope set; grants holding `manage`/`admin` must declare an
    /// `agent_name` (#703).
    ///
    /// Sprint 049: superseded by [`Self::identities`], and kept for the
    /// transition window. **The window is open exactly while this table
    /// is non-empty** — there is no separate flag, because deleting the
    /// rows is what closes it and a second mechanism for one fact is a
    /// second thing to get wrong (sprint 049 D-1; korg:2450 does the
    /// deletion).
    #[serde(default)]
    pub tokens: Vec<TokenGrantConfig>,

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
        if !self.bearer_token.is_empty() {
            errors.push(format!(
                "[auth]: {}",
                AuthConfigError::LegacyBearerTokenRetired
            ));
        }
        // Sprint 049: either table may carry the grants while the
        // transition window is open, so "no grants at all" is the
        // failure — not "no tokens".
        if self.tokens.is_empty() && self.identities.is_empty() {
            errors.push(format!("[auth]: {}", AuthConfigError::NoTokens));
        }
        for (i, g) in self.tokens.iter().enumerate() {
            if let Err(e) = g.validate() {
                errors.push(format!(
                    "[auth.tokens[{i}]] ({label}): {e}",
                    label = g.label.as_deref().unwrap_or("<no label>")
                ));
            }
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

    /// Non-fatal observations about this `[auth]` block. A grant with
    /// no `label` still boots, but its log and metric attribution is
    /// empty, which is worth saying out loud.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = self
            .tokens
            .iter()
            .enumerate()
            .filter(|(_, g)| g.label.is_none())
            .map(|(i, _)| {
                format!("[auth.tokens[{i}]]: no `label` set; log/metric attribution will be empty")
            })
            .collect();
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

/// How the caller proved who they are (sprint 049). Recorded beside the
/// resolved author so the transition window's progress is readable off
/// the logs: while any caller is still `Bearer`, the window cannot
/// close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    /// `X-Homelab-Agent: <agent_name>` matched an `[[auth.identities]]`
    /// row.
    Identity,
    /// `Authorization: Bearer <token>` matched an `[[auth.tokens]]`
    /// grant — the legacy path, live only while the window is open.
    Bearer,
}

impl AuthMethod {
    /// The log/wire spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Bearer => "bearer",
        }
    }
}

impl std::fmt::Display for AuthMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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
    pub method: AuthMethod,
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
        let grant: TokenGrantConfig = toml::from_str(
            r#"
            token      = "abcdefghijklmnop"
            scopes     = ["read", "write", "manage"]
            label      = "claude"
            agent_name = "claude"
            "#,
        )
        .expect("manage must parse as a scope in [[auth.tokens]]");
        assert_eq!(grant.scopes, vec![Scope::Read, Scope::Write, Scope::Manage]);
        grant.validate().unwrap();
    }

    /// Sprint 034 (#703): a grant holding `manage` or `admin` without
    /// an `agent_name` is a config error — privileged actions must be
    /// attributable (the property sprint 025 built and #670 Q4 asked
    /// to close).
    #[test]
    fn privileged_grant_without_agent_name_is_rejected() {
        for privileged in [Scope::Manage, Scope::Admin] {
            let g = TokenGrantConfig {
                token: "abcdefghijklmnop".into(),
                scopes: vec![Scope::Read, Scope::Write, privileged],
                label: Some("unattributed".into()),
                agent_name: None,
            };
            assert!(
                matches!(
                    g.validate(),
                    Err(AuthConfigError::PrivilegedGrantNeedsAgentName)
                ),
                "{privileged} without agent_name must be rejected"
            );
        }
    }

    /// The counterpart boundaries: read/write-only grants stay valid
    /// without an `agent_name` (back-compat for tokens issued before
    /// sprint 009), and a privileged grant WITH one is accepted.
    #[test]
    fn privileged_grant_rule_boundaries() {
        let unprivileged = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: None,
            agent_name: None,
        };
        unprivileged.validate().unwrap();

        let attributed = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write, Scope::Manage, Scope::Admin],
            label: Some("ken-admin".into()),
            agent_name: Some("ken_admin".into()),
        };
        attributed.validate().unwrap();
    }

    #[test]
    fn token_grant_validates_length() {
        let g = TokenGrantConfig {
            token: "short".into(),
            scopes: vec![Scope::Read],
            label: None,
            agent_name: None,
        };
        assert!(matches!(g.validate(), Err(AuthConfigError::TokenTooShort)));
    }

    #[test]
    fn token_grant_requires_scopes() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![],
            label: None,
            agent_name: None,
        };
        assert!(matches!(g.validate(), Err(AuthConfigError::EmptyScopes)));
    }

    #[test]
    fn token_grant_accepts_valid() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: Some("ghcp".into()),
            agent_name: Some("alice".into()),
        };
        g.validate().unwrap();
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

    /// The transition window: a config carrying ONLY identities is
    /// startable (that is what korg:2450 leaves behind), a config
    /// carrying only tokens still is (that is today), and one carrying
    /// neither is not.
    #[test]
    fn either_table_satisfies_the_grant_requirement() {
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

        let tokens_only = AuthConfig {
            tokens: vec![TokenGrantConfig {
                token: "abcdefghijklmnop".into(),
                scopes: vec![Scope::Read],
                label: None,
                agent_name: None,
            }],
            ..AuthConfig::default()
        };
        assert!(tokens_only.errors().is_empty());

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

    /// An `agent_name` shared between a token grant and an identity is
    /// NOT a duplicate — it is the expected state for the whole
    /// transition window, and both resolve to the same author.
    #[test]
    fn same_name_in_both_tables_is_the_transition_window_not_an_error() {
        let cfg = AuthConfig {
            tokens: vec![TokenGrantConfig {
                token: "abcdefghijklmnop".into(),
                scopes: vec![Scope::Read, Scope::Write],
                label: Some("claude".into()),
                agent_name: Some("claude".into()),
            }],
            identities: vec![IdentityConfig {
                agent_name: "claude".into(),
                scopes: vec![Scope::Read, Scope::Write],
                label: Some("claude".into()),
                nodes: vec![],
            }],
            ..AuthConfig::default()
        };
        assert!(cfg.errors().is_empty(), "{:?}", cfg.errors());
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
            ..AuthConfig::default()
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
            method: AuthMethod::Identity,
        };
        assert_eq!(peer.node_or_unknown(), "unknown");
        assert_eq!(peer.method.as_str(), "identity");

        let resolved = AuthenticatedPeer {
            addr: Some("100.97.109.60".parse().unwrap()),
            node: Some("kai".into()),
            method: AuthMethod::Bearer,
        };
        assert_eq!(resolved.node_or_unknown(), "kai");
        assert_eq!(resolved.method.as_str(), "bearer");
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
    fn token_grant_rejects_invalid_agent_name() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read],
            label: None,
            agent_name: Some("Alice".into()),
        };
        let err = g.validate().unwrap_err();
        match err {
            AuthConfigError::InvalidAgentName { reason } => {
                assert_eq!(reason, AgentNameInvalidReason::Charset);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
