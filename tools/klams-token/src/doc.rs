//! Format-preserving structural access to the `[[auth.identities]]`
//! rows.
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
//! The schema, though, is not this module's to invent: rows are read
//! back through [`klams_types::AuthConfig`], the exact type
//! `klams-service` boots from.
//!
//! Sprint 052 deleted the `[[auth.tokens]]` half. `GrantsDoc` and
//! `GrantFingerprint` keep their names: the fingerprint guard is
//! table-agnostic and renaming it would be churn across the writer for
//! no behaviour.

use anyhow::{anyhow, bail, Context, Result};
use klams_types::{AuthConfig, IdentityConfig, Scope};
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

/// One `[[auth.identities]]` row, as the CLI presents it.
#[derive(Debug, Clone)]
pub struct IdentityView {
    pub index: usize,
    pub label: Option<String>,
    pub agent_name: String,
    pub scopes: Vec<Scope>,
    pub nodes: Vec<String>,
}

impl IdentityView {
    /// The row's identity — always the `agent_name`, because that is
    /// the key rather than a fallback.
    #[must_use]
    pub fn identity(&self) -> String {
        self.agent_name.clone()
    }

    #[must_use]
    pub fn fingerprint(&self) -> GrantFingerprint {
        GrantFingerprint::identity(self.agent_name.clone())
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

    /// The `[[auth.identities]]` rows.
    ///
    /// # Errors
    /// If `[auth]` does not deserialize.
    pub fn identities(&self) -> Result<Vec<IdentityView>> {
        Ok(self
            .auth()?
            .identities
            .into_iter()
            .enumerate()
            .map(|(index, i)| IdentityView {
                index,
                label: i.label,
                agent_name: i.agent_name,
                scopes: i.scopes,
                nodes: i.nodes,
            })
            .collect())
    }

    /// # Errors
    /// If `[auth]` does not deserialize.
    pub fn identity_fingerprints(&self) -> Result<Vec<GrantFingerprint>> {
        Ok(self
            .identities()?
            .iter()
            .map(IdentityView::fingerprint)
            .collect())
    }

    /// Resolve a `<selector>` — an `agent_name` or a `label` — to one
    /// identity row.
    ///
    /// # Errors
    /// If nothing matches, or more than one does. Same refusal as
    /// [`Self::find`], for the same reason.
    pub fn find_identity(&self, selector: &str) -> Result<IdentityView> {
        let identities = self.identities()?;
        let matches: Vec<&IdentityView> = identities
            .iter()
            .filter(|i| i.agent_name == selector || i.label.as_deref() == Some(selector))
            .collect();
        match matches.as_slice() {
            [one] => Ok((*one).clone()),
            [] => {
                let known: Vec<String> = identities.iter().map(IdentityView::identity).collect();
                bail!(
                    "no identity matches `{selector}` (matched against agent_name and label)\n\
                     known identities: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            }
            many => bail!(
                "`{selector}` matches {} identities (indices {}) — refusing to guess which one \
                 you meant; disambiguate with the agent_name",
                many.len(),
                many.iter()
                    .map(|i| i.index.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Append a new `[[auth.identities]]` block. Never
    /// touches an existing one.
    ///
    /// # Errors
    /// If `[auth]` exists but is not a table, or `auth.identities`
    /// exists but is not an array of tables.
    pub fn add_identity(&mut self, id: &IdentityConfig) -> Result<()> {
        let mut table = Table::new();
        table["agent_name"] = value(id.agent_name.clone());
        let mut scopes = Array::new();
        for s in &id.scopes {
            scopes.push(s.as_str());
        }
        table["scopes"] = value(scopes);
        if let Some(label) = &id.label {
            table["label"] = value(label.clone());
        }
        if !id.nodes.is_empty() {
            let mut nodes = Array::new();
            for n in &id.nodes {
                nodes.push(n.as_str());
            }
            table["nodes"] = value(nodes);
        }

        // Same positioning rule as `add`: render the block beside its
        // siblings rather than after `[postgres]`.
        let insert_at = self.next_position_in("identities");
        shift_positions_from(self.doc.as_table_mut(), insert_at);
        table.set_position(insert_at);

        self.auth_array_mut("identities")?.push(table);
        Ok(())
    }

    /// Delete the identity at `index`.
    ///
    /// # Errors
    /// If `auth.identities` is missing or `index` is out of range.
    pub fn remove_identity(&mut self, index: usize) -> Result<()> {
        let array = self.auth_array_mut("identities")?;
        if index >= array.len() {
            bail!(
                "identity index {index} is out of range ({} identities)",
                array.len()
            );
        }
        array.remove(index);
        Ok(())
    }

    /// Replace one identity's `scopes`, touching nothing else.
    ///
    /// # Errors
    /// If `auth.identities` is missing or `index` is out of range.
    pub fn set_identity_scopes(&mut self, index: usize, scopes: &[Scope]) -> Result<()> {
        let mut array = Array::new();
        for s in scopes {
            array.push(s.as_str());
        }
        self.set_field_in("identities", index, "scopes", value(array))
    }

    /// Replace one identity's `nodes` pin list, touching nothing else.
    /// An empty list removes the key entirely rather than leaving
    /// `nodes = []`, which reads like a pin to nowhere.
    ///
    /// # Errors
    /// If `auth.identities` is missing or `index` is out of range.
    pub fn set_identity_nodes(&mut self, index: usize, nodes: &[String]) -> Result<()> {
        if nodes.is_empty() {
            let array = self.auth_array_mut("identities")?;
            let table = array
                .get_mut(index)
                .ok_or_else(|| anyhow!("identity index {index} is out of range"))?;
            table.remove("nodes");
            return Ok(());
        }
        let mut array = Array::new();
        for n in nodes {
            array.push(n.as_str());
        }
        self.set_field_in("identities", index, "nodes", value(array))
    }

    fn set_field_in(&mut self, array_key: &str, index: usize, key: &str, new: Item) -> Result<()> {
        let array = self.auth_array_mut(array_key)?;
        let table = array
            .get_mut(index)
            .ok_or_else(|| anyhow!("{array_key} index {index} is out of range"))?;

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

    fn auth_array_mut(&mut self, key: &str) -> Result<&mut toml_edit::ArrayOfTables> {
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
        let array = auth
            .entry(key)
            .or_insert_with(|| Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        array.as_array_of_tables_mut().ok_or_else(|| {
            anyhow!("`auth.{key}` exists but is not an array of `[[auth.{key}]]` tables")
        })
    }

    /// Where a new identity block should be rendered: right after the
    /// last existing one, else just inside `[auth]`, else at the end of
    /// the document.
    fn next_position_in(&self, key: &str) -> usize {
        // After the last block of this kind, when there is one.
        if let Some(p) = self.last_position_of(key) {
            return p + 1;
        }
        // Otherwise immediately after the `[auth]` table itself.
        // Without this, adding the very first `[[auth.identities]]` to
        // a config that has none falls through to end-of-document and
        // renders below `[postgres]` — valid TOML that reads exactly
        // like the file was mangled, which is the failure this
        // positioning exists to avoid. Measured twice: once as the
        // first CLI run of `identity add` (sprint 049, which anchored
        // on the `[[auth.tokens]]` array), and again in sprint 052 when
        // deleting that array took the anchor with it.
        if let Some(p) = self
            .doc
            .get("auth")
            .and_then(Item::as_table)
            .and_then(Table::position)
        {
            return p + 1;
        }
        max_position(self.doc.as_table()) + 1
    }

    fn last_position_of(&self, key: &str) -> Option<usize> {
        self.doc
            .get("auth")
            .and_then(Item::as_table)
            .and_then(|t| t.get(key))
            .and_then(Item::as_array_of_tables)
            .and_then(|aot| aot.iter().filter_map(Table::position).max())
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
# Declared identities. SCOPES ARE FLAT, NOT HIERARCHICAL.

[[auth.identities]]
agent_name = "claude"
scopes     = ["read", "write", "manage"]
label      = "claude"

[[auth.identities]]
agent_name = "kmon"
scopes     = ["read", "write"]
label      = "kmon"
nodes      = ["kubs0"]

[postgres]
url = "postgres://localhost/klams"
"#;

    #[test]
    fn reads_identities_through_the_service_schema() {
        let doc = GrantsDoc::parse(FIXTURE).unwrap();
        let ids = doc.identities().unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].agent_name, "claude");
        assert_eq!(
            ids[0].scopes,
            vec![Scope::Read, Scope::Write, Scope::Manage]
        );
        assert!(ids[0].nodes.is_empty());
        assert_eq!(ids[1].nodes, vec!["kubs0".to_string()]);
    }

    #[test]
    fn finds_an_identity_by_agent_name_or_label() {
        let doc = GrantsDoc::parse(FIXTURE).unwrap();
        assert_eq!(doc.find_identity("kmon").unwrap().index, 1);
        assert!(doc.find_identity("nope").is_err());
    }

    /// The comments in this file ARE the operator documentation, so an
    /// edit that dropped them would be a regression even though the
    /// TOML stayed valid.
    #[test]
    fn adding_an_identity_preserves_comments_and_siblings() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.add_identity(&IdentityConfig {
            agent_name: "klams-mind-eval".into(),
            scopes: vec![Scope::Read],
            label: Some("klams-mind-eval".into()),
            nodes: vec![],
        })
        .unwrap();
        let out = doc.to_string();
        assert!(out.contains("SCOPES ARE FLAT"), "comments must survive");
        assert!(out.contains("klams-mind-eval"));

        let reparsed = GrantsDoc::parse(&out).unwrap();
        assert_eq!(reparsed.identities().unwrap().len(), 3);
        // The existing identities did not move.
        assert_eq!(reparsed.identities().unwrap()[0].agent_name, "claude");

        // And the new block renders beside its siblings, not after
        // [postgres] — the failure that reads like a mangled file.
        let mind = out.find("klams-mind-eval").unwrap();
        let postgres = out.find("[postgres]").unwrap();
        assert!(mind < postgres, "new identity rendered below [postgres]");
    }

    #[test]
    fn identity_scopes_and_nodes_edit_only_their_own_row() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.set_identity_scopes(0, &[Scope::Read]).unwrap();
        doc.set_identity_nodes(0, &["kai".to_string(), "kubs0".to_string()])
            .unwrap();
        let reparsed = GrantsDoc::parse(&doc.to_string()).unwrap();
        let ids = reparsed.identities().unwrap();
        assert_eq!(ids[0].scopes, vec![Scope::Read]);
        assert_eq!(ids[0].nodes, vec!["kai".to_string(), "kubs0".to_string()]);
        // The sibling is byte-for-byte what it was.
        assert_eq!(ids[1].scopes, vec![Scope::Read, Scope::Write]);
        assert_eq!(ids[1].nodes, vec!["kubs0".to_string()]);
    }

    /// Unpinning removes the key rather than leaving `nodes = []`,
    /// which reads like a pin to nowhere.
    #[test]
    fn unpinning_removes_the_nodes_key() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.set_identity_nodes(1, &[]).unwrap();
        let out = doc.to_string();
        assert!(!out.contains("nodes"), "{out}");
        assert!(GrantsDoc::parse(&out).unwrap().identities().unwrap()[1]
            .nodes
            .is_empty());
    }

    #[test]
    fn removing_an_identity_takes_exactly_one_row() {
        let mut doc = GrantsDoc::parse(FIXTURE).unwrap();
        doc.remove_identity(0).unwrap();
        let reparsed = GrantsDoc::parse(&doc.to_string()).unwrap();
        assert_eq!(reparsed.identities().unwrap().len(), 1);
        assert_eq!(reparsed.identities().unwrap()[0].agent_name, "kmon");
    }

    /// A file with no `[[auth.identities]]` at all grows the array
    /// rather than failing.
    #[test]
    fn adding_the_first_identity_to_a_config_that_has_none() {
        const NO_IDENTITIES: &str = r#"# klams-service runtime configuration.

[server]
listen_addr = "127.0.0.1"
port = 7777

[auth]
# Scoped identities. SCOPES ARE FLAT, NOT HIERARCHICAL.

[postgres]
url = "postgres://localhost/klams"
"#;
        let mut doc = GrantsDoc::parse(NO_IDENTITIES).unwrap();
        assert!(doc.identities().unwrap().is_empty());
        doc.add_identity(&IdentityConfig {
            agent_name: "claude".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: Some("claude".into()),
            nodes: vec![],
        })
        .unwrap();
        let out = doc.to_string();
        let reparsed = GrantsDoc::parse(&out).unwrap();
        assert_eq!(reparsed.identities().unwrap().len(), 1);
        assert!(out.contains("SCOPES ARE FLAT"), "comments must survive");
        // The block belongs inside `[auth]`, not after `[postgres]`.
        // Measured failure: without the sibling-array fallback in
        // `next_position_in`, the very first `identity add` rendered
        // the block at the end of the file — valid TOML that reads
        // exactly like the file was mangled.
        let identity_at = out.find("[[auth.identities]]").unwrap();
        let postgres_at = out.find("[postgres]").unwrap();
        assert!(
            identity_at < postgres_at,
            "first identity rendered below [postgres]:\n{out}"
        );
    }

    /// An identity fingerprint is its name — there is no token to
    /// digest — so the set-level guard is over names.
    #[test]
    fn identity_fingerprints_are_names() {
        let doc = GrantsDoc::parse(FIXTURE).unwrap();
        let fps = doc.identity_fingerprints().unwrap();
        assert_eq!(fps.len(), 2);
        assert_eq!(fps[0].key, "claude");
        assert_eq!(fps[0].token, crate::fingerprint::NO_TOKEN);
    }
}
