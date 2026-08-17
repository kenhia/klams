//! Format-preserving structural access to the `[[auth.tokens]]` grants.
//!
//! Editing goes through `toml_edit` rather than a serde round-trip for
//! two reasons. The obvious one is that the live `klams.toml` is
//! heavily commented and those comments are the operator documentation
//! for the auth model — a round-trip through `Config` would delete all
//! of them. The less obvious one is that a serde round-trip also
//! *materializes* every `#[serde(default)]` in the service's config
//! tree, so the file would silently grow a frozen copy of today's
//! defaults and stop tracking them.
//!
//! The schema, though, is not this module's to invent: grants are read
//! back through [`klams_types::AuthConfig`], the exact type
//! `klams-service` boots from.

use anyhow::{anyhow, bail, Context, Result};
use klams_types::{AuthConfig, Scope, TokenGrantConfig};
use serde::Deserialize;
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::fingerprint::GrantFingerprint;

/// Just enough of `klams.toml` to reach `[auth]`. Every other block is
/// ignored on purpose — this tool has no business parsing (or being
/// able to fail on) the postgres, qdrant or embeddings blocks.
#[derive(Debug, Deserialize)]
struct AuthSlice {
    #[serde(default)]
    auth: AuthConfig,
}

/// One grant, as the CLI presents it.
#[derive(Debug, Clone)]
pub struct GrantView {
    pub index: usize,
    pub label: Option<String>,
    pub agent_name: Option<String>,
    pub scopes: Vec<Scope>,
    pub token: String,
}

impl GrantView {
    /// The grant's identity, as klams itself keys it: `agent_name`
    /// first, because that is what memories are attributed to. Falls
    /// back to `label`, then to the positional index, so an unnamed
    /// grant is still addressable and still fingerprintable.
    #[must_use]
    pub fn identity(&self) -> String {
        self.agent_name
            .clone()
            .or_else(|| self.label.clone())
            .unwrap_or_else(|| format!("#{}", self.index))
    }

    #[must_use]
    pub fn fingerprint(&self) -> GrantFingerprint {
        GrantFingerprint::new(self.identity(), &self.token)
    }

    #[must_use]
    pub fn scope_list(&self) -> String {
        self.scopes
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// A parsed `klams.toml` that can be edited without losing its shape.
#[derive(Debug, Clone)]
pub struct GrantsDoc {
    doc: DocumentMut,
}

impl GrantsDoc {
    /// # Errors
    /// If the text is not valid TOML, or its `[auth]` block does not
    /// match the schema `klams-service` expects.
    pub fn parse(text: &str) -> Result<Self> {
        let doc: DocumentMut = text.parse().context("parsing klams.toml")?;
        let parsed = Self { doc };
        // Fail here rather than at the first edit: a file we cannot
        // read as an `[auth]` block is one we must not write back.
        parsed.auth()?;
        Ok(parsed)
    }

    /// The `[auth]` block, as `klams-service` would load it.
    ///
    /// # Errors
    /// If `[auth]` does not deserialize into [`AuthConfig`].
    pub fn auth(&self) -> Result<AuthConfig> {
        let slice: AuthSlice = toml::from_str(&self.doc.to_string()).context(
            "reading the [auth] block (does it match the schema klams-service expects?)",
        )?;
        Ok(slice.auth)
    }

    /// # Errors
    /// If `[auth]` does not deserialize.
    pub fn grants(&self) -> Result<Vec<GrantView>> {
        Ok(self
            .auth()?
            .tokens
            .into_iter()
            .enumerate()
            .map(|(index, g)| GrantView {
                index,
                label: g.label,
                agent_name: g.agent_name,
                scopes: g.scopes,
                token: g.token,
            })
            .collect())
    }

    /// # Errors
    /// If `[auth]` does not deserialize.
    pub fn fingerprints(&self) -> Result<Vec<GrantFingerprint>> {
        Ok(self.grants()?.iter().map(GrantView::fingerprint).collect())
    }

    /// Resolve a `<selector>` — an `agent_name` or a `label` — to one
    /// grant.
    ///
    /// # Errors
    /// If nothing matches, or if more than one does. Both are refusals
    /// rather than a "pick the first" guess: this tool exists because
    /// somebody once edited the wrong grant.
    pub fn find(&self, selector: &str) -> Result<GrantView> {
        let grants = self.grants()?;
        let matches: Vec<&GrantView> = grants
            .iter()
            .filter(|g| {
                g.agent_name.as_deref() == Some(selector) || g.label.as_deref() == Some(selector)
            })
            .collect();
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => {
                let known: Vec<String> = grants.iter().map(GrantView::identity).collect();
                bail!(
                    "no grant matches `{selector}` (matched against agent_name and label)\n\
                     known grants: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            }
            many => bail!(
                "`{selector}` matches {} grants (indices {}) — refusing to guess which one you \
                 meant; disambiguate with the agent_name",
                many.len(),
                many.iter()
                    .map(|g| g.index.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Append a new `[[auth.tokens]]` block. Never touches an existing
    /// one — the array is only ever pushed to.
    ///
    /// # Errors
    /// If `[auth]` exists but is not a table, or `auth.tokens` exists
    /// but is not an array of tables.
    pub fn add(&mut self, grant: &TokenGrantConfig) -> Result<()> {
        let mut table = Table::new();
        table["token"] = value(grant.token.clone());
        let mut scopes = Array::new();
        for s in &grant.scopes {
            scopes.push(s.as_str());
        }
        table["scopes"] = value(scopes);
        if let Some(label) = &grant.label {
            table["label"] = value(label.clone());
        }
        if let Some(agent) = &grant.agent_name {
            table["agent_name"] = value(agent.clone());
        }

        // Render the new block next to its siblings rather than at the
        // end of the file. toml_edit emits tables in `position` order
        // and puts unpositioned ones last, so a plain push would drop
        // the grant below `[postgres]` — valid TOML that reads exactly
        // like the file was mangled.
        let insert_at = self.next_grant_position();
        shift_positions_from(self.doc.as_table_mut(), insert_at);
        table.set_position(insert_at);

        self.tokens_array_mut()?.push(table);
        Ok(())
    }

    /// Delete the grant at `index`.
    ///
    /// # Errors
    /// If `auth.tokens` is missing or `index` is out of range.
    pub fn remove(&mut self, index: usize) -> Result<()> {
        let array = self.tokens_array_mut()?;
        if index >= array.len() {
            bail!(
                "grant index {index} is out of range ({} grants)",
                array.len()
            );
        }
        array.remove(index);
        Ok(())
    }

    /// Replace one grant's `scopes` array, touching nothing else.
    ///
    /// # Errors
    /// If `auth.tokens` is missing or `index` is out of range.
    pub fn set_scopes(&mut self, index: usize, scopes: &[Scope]) -> Result<()> {
        let mut array = Array::new();
        for s in scopes {
            array.push(s.as_str());
        }
        self.set_field(index, "scopes", value(array))
    }

    /// Replace one grant's `token` value, touching nothing else — not
    /// its `agent_name`, which is the identity klams attributes that
    /// agent's memories to.
    ///
    /// # Errors
    /// If `auth.tokens` is missing or `index` is out of range.
    pub fn set_token(&mut self, index: usize, token: &str) -> Result<()> {
        self.set_field(index, "token", value(token))
    }

    fn set_field(&mut self, index: usize, key: &str, new: Item) -> Result<()> {
        let array = self.tokens_array_mut()?;
        let table = array
            .get_mut(index)
            .ok_or_else(|| anyhow!("grant index {index} is out of range"))?;

        // Carry the old value's decor across so a trailing comment on
        // the line ("# rotated after the 401") survives the edit.
        let decor = table
            .get(key)
            .and_then(Item::as_value)
            .map(|v| v.decor().clone());
        table[key] = new;
        if let (Some(decor), Some(v)) = (decor, table[key].as_value_mut()) {
            *v.decor_mut() = decor;
        }
        Ok(())
    }

    fn tokens_array_mut(&mut self) -> Result<&mut toml_edit::ArrayOfTables> {
        let auth = self
            .doc
            .entry("auth")
            .or_insert_with(|| Item::Table(Table::new()));
        if auth.is_none() {
            *auth = Item::Table(Table::new());
        }
        let auth = auth
            .as_table_mut()
            .ok_or_else(|| anyhow!("`auth` exists but is not a table"))?;
        let tokens = auth
            .entry("tokens")
            .or_insert_with(|| Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        tokens.as_array_of_tables_mut().ok_or_else(|| {
            anyhow!("`auth.tokens` exists but is not an array of `[[auth.tokens]]` tables")
        })
    }

    /// Where a new grant block should be rendered: right after the last
    /// existing one, or at the end of the document if there are none.
    fn next_grant_position(&self) -> usize {
        let existing_max = self
            .doc
            .get("auth")
            .and_then(Item::as_table)
            .and_then(|t| t.get("tokens"))
            .and_then(Item::as_array_of_tables)
            .and_then(|aot| aot.iter().filter_map(Table::position).max());
        match existing_max {
            Some(p) => p + 1,
            None => max_position(self.doc.as_table()) + 1,
        }
    }
}

impl std::fmt::Display for GrantsDoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.doc)
    }
}

/// Make room at `from` by pushing every table at or after it down one.
fn shift_positions_from(table: &mut Table, from: usize) {
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(t) => {
                if let Some(p) = t.position() {
                    if p >= from {
                        t.set_position(p + 1);
                    }
                }
                shift_positions_from(t, from);
            }
            Item::ArrayOfTables(aot) => {
                for t in aot.iter_mut() {
                    if let Some(p) = t.position() {
                        if p >= from {
                            t.set_position(p + 1);
                        }
                    }
                    shift_positions_from(t, from);
                }
            }
            _ => {}
        }
    }
}

fn max_position(table: &Table) -> usize {
    let mut max = 0;
    for (_, item) in table {
        match item {
            Item::Table(t) => {
                max = max.max(t.position().unwrap_or(0)).max(max_position(t));
            }
            Item::ArrayOfTables(aot) => {
                for t in aot {
                    max = max.max(t.position().unwrap_or(0)).max(max_position(t));
                }
            }
            _ => {}
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"# klams-service runtime configuration.

[server]
listen_addr = "127.0.0.1"
port = 7777

[auth]
# Scoped grants. SCOPES ARE FLAT, NOT HIERARCHICAL.

# The dashboard only reads.
[[auth.tokens]]
token      = "klams-view-0123456789abcdef"
scopes     = ["read"]
label      = "klams-view"
agent_name = "klams-view"

[[auth.tokens]]
token      = "bench-0123456789abcdef"   # provisioned 2026-03
scopes     = ["read", "write"]
label      = "bench"
agent_name = "klams-bench"

[postgres]
url = "postgres://localhost/klams"
"#;

    #[test]
    fn reads_grants_through_the_service_schema() {
        let doc = GrantsDoc::parse(FIXTURE).unwrap();
        let grants = doc.grants().unwrap();
        assert_eq!(grants.len(), 2);
        assert_eq!(grants[0].agent_name.as_deref(), Some("klams-view"));
        assert_eq!(grants[1].scopes, vec![Scope::Read, Scope::Write]);
    }

    #[test]
    fn identity_prefers_agent_name_then_label() {
        let doc = GrantsDoc::parse(
            r#"
[[auth.tokens]]
token = "labelled-0123456789abcdef"
scopes = ["read"]
label = "just-a-label"

[[auth.tokens]]
token = "anonymous-0123456789abcdef"
scopes = ["read"]
"#,
        )
        .unwrap();
        let grants = doc.grants().unwrap();
        assert_eq!(grants[0].identity(), "just-a-label");
        assert_eq!(grants[1].identity(), "#1");
    }

    /// The whole reason for `toml_edit`: the comments in this file are
    /// the operator documentation for the auth model.
    #[test]
    fn an_edit_preserves_comments_and_formatting() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.set_scopes(0, &[Scope::Read, Scope::Write]).unwrap();
        let out = doc.to_string();
        assert!(out.contains("# klams-service runtime configuration."));
        assert!(out.contains("# SCOPES ARE FLAT, NOT HIERARCHICAL.".trim_start_matches("# ")));
        assert!(out.contains("# The dashboard only reads."));
        assert!(out.contains(r#"token      = "klams-view-0123456789abcdef""#));
        // Including the `=` alignment, which lives in the key's decor.
        assert!(
            out.contains(r#"scopes     = ["read", "write"]"#),
            "scopes edit lost the file's alignment:\n{out}"
        );
    }

    #[test]
    fn a_trailing_comment_survives_a_token_rotation() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.set_token(1, "bench-fedcba98765432100000").unwrap();
        let out = doc.to_string();
        assert!(
            out.contains(r#"= "bench-fedcba98765432100000"   # provisioned 2026-03"#),
            "trailing comment lost:\n{out}"
        );
    }

    #[test]
    fn setting_a_token_leaves_the_agent_name_alone() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.set_token(1, "bench-fedcba98765432100000").unwrap();
        let grants = doc.grants().unwrap();
        assert_eq!(grants[1].agent_name.as_deref(), Some("klams-bench"));
        assert_eq!(grants[1].label.as_deref(), Some("bench"));
        assert_eq!(grants[1].scopes, vec![Scope::Read, Scope::Write]);
    }

    /// A new block belongs next to its siblings, not after `[postgres]`
    /// at the bottom of the file.
    #[test]
    fn add_renders_the_new_block_beside_the_existing_grants() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.add(&TokenGrantConfig {
            token: "krot-0123456789abcdef0000".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: Some("krot".into()),
            agent_name: Some("krot".into()),
        })
        .unwrap();
        let out = doc.to_string();

        let new_block = out.find("krot-0123456789abcdef0000").expect("new grant");
        let postgres = out.find("[postgres]").expect("postgres block");
        assert!(
            new_block < postgres,
            "new grant rendered after [postgres]:\n{out}"
        );

        // And it round-trips through the service's own schema.
        let reparsed = GrantsDoc::parse(&out).unwrap();
        assert_eq!(reparsed.grants().unwrap().len(), 3);
    }

    #[test]
    fn add_creates_the_auth_block_when_the_file_has_none() {
        let mut doc = GrantsDoc::parse("[server]\nport = 7777\n").unwrap();
        doc.add(&TokenGrantConfig {
            token: "first-0123456789abcdef00".into(),
            scopes: vec![Scope::Read],
            label: Some("first".into()),
            agent_name: Some("first".into()),
        })
        .unwrap();
        let reparsed = GrantsDoc::parse(&doc.to_string()).unwrap();
        assert_eq!(reparsed.grants().unwrap().len(), 1);
    }

    #[test]
    fn remove_takes_exactly_one_block() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.remove(0).unwrap();
        let out = doc.to_string();
        assert!(!out.contains("klams-view-0123456789abcdef"));
        assert!(out.contains("bench-0123456789abcdef"));
        assert!(out.contains("[postgres]"));
    }

    #[test]
    fn find_matches_agent_name_or_label() {
        let doc = GrantsDoc::parse(FIXTURE).unwrap();
        assert_eq!(doc.find("klams-bench").unwrap().index, 1);
        assert_eq!(doc.find("bench").unwrap().index, 1);
    }

    #[test]
    fn find_refuses_an_unknown_selector_and_names_what_exists() {
        let doc = GrantsDoc::parse(FIXTURE).unwrap();
        let err = doc.find("nope").unwrap_err().to_string();
        assert!(err.contains("no grant matches `nope`"));
        assert!(err.contains("klams-bench"));
    }

    #[test]
    fn find_refuses_to_guess_between_two_matches() {
        let doc = GrantsDoc::parse(
            r#"
[[auth.tokens]]
token = "one-0123456789abcdef0000"
scopes = ["read"]
label = "shared"
agent_name = "one"

[[auth.tokens]]
token = "two-0123456789abcdef0000"
scopes = ["read"]
label = "shared"
agent_name = "two"
"#,
        )
        .unwrap();
        let err = doc.find("shared").unwrap_err().to_string();
        assert!(err.contains("matches 2 grants"), "{err}");
    }

    #[test]
    fn a_file_whose_auth_block_does_not_match_the_schema_is_refused_up_front() {
        let err = GrantsDoc::parse(
            r#"
[[auth.tokens]]
token = "fine-0123456789abcdef00"
scopes = "read"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("[auth] block"), "{err}");
    }
}
