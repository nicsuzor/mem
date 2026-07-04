//! Project-slug registry backed by the shared `polecat.yaml`.
//!
//! "Project" on a task is an operational routing slug: which
//! polecat-registered repo (or directory) the task's work belongs to. The
//! registry of permissible slugs lives in `polecat.yaml` — the same file the
//! academicOps polecat launcher reads (`aops-core/lib/polecat_config.py`).
//! This module parses only the two blocks mem cares about (`projects:` and
//! `project_aliases:`) and ignores every other key in that file
//! (`session_defaults`, `gates`, `docker`, `polecat_home`, …).
//!
//! Registry schema (superset of the academicOps one):
//!
//! ```yaml
//! projects:
//!   aops:                       # map key — canonical slug by default
//!     repo: academicOps         # ignored by mem (consumed by polecat)
//!     slug: aops                # optional override: canonical slug if present
//!     aliases: [academicOps, acaops]
//! project_aliases:
//!   ao: aops                    # flat top-level alias -> slug shorthand
//! ```
//!
//! File location, first match wins:
//! 1. `<pkb_root>/polecat.yaml` — colocated with the PKB root. Checked first
//!    so test fixtures are deterministic regardless of ambient env vars; the
//!    accepted tradeoff is that a stray polecat.yaml committed into a real
//!    PKB root shadows the registry named by the env vars below.
//! 2. `$AOPS_POLECAT_CONFIG` — explicit path, same env var as the Python SSoT.
//! 3. `$AOPS_SESSIONS/polecat.yaml` — the Python loader's host default.
//!
//! When no file is locatable, any explicit non-builtin `project` value is a
//! hard error (mem cannot vouch for a slug it cannot check). The builtin
//! slugs in [`BUILTIN_PROJECT_SLUGS`] are always valid with no lookup.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Framework-reserved slugs that are always valid without a registry lookup:
/// `task` is the projectless ID-prefix fallback; `adhoc-sessions` is the
/// catch-all parent for ad-hoc session tasks.
pub const BUILTIN_PROJECT_SLUGS: &[&str] = &["task", "adhoc-sessions"];

#[derive(Debug, Default, serde::Deserialize)]
struct RawPolecatYaml {
    #[serde(default)]
    projects: HashMap<String, RawProjectEntry>,
    #[serde(default)]
    project_aliases: HashMap<String, String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct RawProjectEntry {
    /// Optional canonical-slug override; the map key is used when absent.
    #[serde(default)]
    slug: Option<String>,
    /// Alternate names accepted for this project.
    #[serde(default)]
    aliases: Vec<String>,
}

// serde ignores unknown fields by default, so the rest of the shared
// polecat.yaml (repo:, default_branch:, gates:, docker:, …) parses cleanly
// without being modeled here.

/// Parsed project registry: a case-insensitive lookup from any accepted name
/// (map key, `slug:`, per-project alias, top-level alias) to the canonical slug.
#[derive(Debug, Clone)]
pub struct PolecatRegistry {
    /// lowercase accepted name -> canonical slug
    lookup: HashMap<String, String>,
    /// canonical slugs, sorted, for error messages
    slugs: Vec<String>,
    /// where the registry was loaded from (error messages / logging)
    pub source_path: PathBuf,
}

impl PolecatRegistry {
    /// Locate and load the registry. `Ok(None)` when no polecat.yaml is
    /// locatable (not configured); `Err` when a file was found but cannot be
    /// read or parsed (fail-fast — a malformed registry is a config bug).
    pub fn load(root: &Path) -> Result<Option<Self>> {
        match locate_config(root) {
            Some(path) => Self::load_from(&path).map(Some),
            None => Ok(None),
        }
    }

    /// Load and parse a specific polecat.yaml file.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read polecat.yaml at {}", path.display()))?;
        let raw: RawPolecatYaml = serde_yaml::from_str(&content)
            .with_context(|| format!("malformed polecat.yaml at {}", path.display()))?;

        let mut lookup: HashMap<String, String> = HashMap::new();
        let mut slugs: Vec<String> = Vec::new();

        for (key, entry) in &raw.projects {
            let canonical = entry
                .slug
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(key.as_str())
                .to_string();
            slugs.push(canonical.clone());
            // The map key is always an accepted name, even when slug: overrides it.
            lookup.insert(key.to_lowercase(), canonical.clone());
            lookup.insert(canonical.to_lowercase(), canonical.clone());
            for alias in &entry.aliases {
                let alias = alias.trim();
                if !alias.is_empty() {
                    lookup.insert(alias.to_lowercase(), canonical.clone());
                }
            }
        }

        // Top-level shorthand aliases: alias -> project map key (or slug).
        // Resolve the target through the project lookup so `ao: aops` still
        // lands on the canonical slug even if aops' entry has a slug: override.
        // A target that doesn't resolve is a config bug (e.g. `ao: aopps`
        // typo) — fail fast rather than silently minting a bogus slug.
        for (alias, target) in &raw.project_aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                continue;
            }
            let canonical = lookup
                .get(&target.trim().to_lowercase())
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "malformed polecat.yaml at {}: project_aliases entry \
                         '{}: {}' points at a project that does not exist in \
                         the projects: block",
                        path.display(),
                        alias,
                        target
                    )
                })?;
            lookup.insert(alias.to_lowercase(), canonical);
        }

        slugs.sort();
        slugs.dedup();

        Ok(Self {
            lookup,
            slugs,
            source_path: path.to_path_buf(),
        })
    }

    /// Case-insensitive resolve of any accepted name to its canonical slug.
    pub fn resolve(&self, input: &str) -> Option<String> {
        self.lookup.get(&input.trim().to_lowercase()).cloned()
    }

    /// Sorted canonical slugs, for error messages.
    pub fn known_slugs(&self) -> &[String] {
        &self.slugs
    }
}

/// Resolve a caller-supplied project value to its canonical slug, validating
/// it against the registry. This is the single entry point write paths call.
///
/// Builtin slugs pass through with no registry lookup. Otherwise the value
/// must resolve against a locatable polecat.yaml; a missing registry or an
/// unregistered value is a hard error.
///
/// Callers that validate one value against many documents (batch update)
/// should instead call [`PolecatRegistry::load`] once and use
/// [`resolve_with`] per value.
pub fn resolve_project(root: &Path, input: &str) -> Result<String> {
    if let Some(builtin) = builtin_slug(input) {
        return Ok(builtin);
    }
    let registry = PolecatRegistry::load(root)?;
    resolve_with(registry.as_ref(), input)
}

/// Resolve `input` against an already-loaded registry (`None` = no registry
/// locatable). Shared by `resolve_project` and load-once batch callers.
pub fn resolve_with(registry: Option<&PolecatRegistry>, input: &str) -> Result<String> {
    if let Some(builtin) = builtin_slug(input) {
        return Ok(builtin);
    }
    match registry {
        None => anyhow::bail!(
            "project '{input}' cannot be validated: no polecat.yaml found \
             (checked <pkb_root>/polecat.yaml, $AOPS_POLECAT_CONFIG, \
             $AOPS_SESSIONS/polecat.yaml). Register the project or omit the \
             field to use the 'task' default."
        ),
        Some(reg) => reg.resolve(input).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown project '{}': not registered in {}. Known slugs: {}",
                input,
                reg.source_path.display(),
                reg.known_slugs().join(", ")
            )
        }),
    }
}

/// Case-insensitive builtin match, normalized to canonical casing.
fn builtin_slug(input: &str) -> Option<String> {
    let t = input.trim();
    BUILTIN_PROJECT_SLUGS
        .iter()
        .find(|b| b.eq_ignore_ascii_case(t))
        .map(|b| b.to_string())
}

/// Locate polecat.yaml. First match wins; see module docs for the order.
fn locate_config(root: &Path) -> Option<PathBuf> {
    let local = root.join("polecat.yaml");
    if local.is_file() {
        return Some(local);
    }
    if let Ok(explicit) = std::env::var("AOPS_POLECAT_CONFIG") {
        let p = PathBuf::from(explicit);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(sessions) = std::env::var("AOPS_SESSIONS") {
        let p = PathBuf::from(sessions).join("polecat.yaml");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_yaml(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("polecat.yaml");
        std::fs::write(&path, content).unwrap();
        path
    }

    const CANONICAL: &str = r#"
polecat_home: ~/.polecat
session_defaults:
  claude_model: whatever
  gates:
    handover: block
projects:
  aops:
    repo: academicOps
    default_branch: main
    aliases: [academicOps, acaops]
  sessions:
    repo: sessions
  mem: {}
project_aliases:
  ao: aops
"#;

    #[test]
    fn resolves_map_key_aliases_and_shorthand() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml(dir.path(), CANONICAL);
        let reg = PolecatRegistry::load(dir.path()).unwrap().unwrap();

        assert_eq!(reg.resolve("aops").as_deref(), Some("aops"));
        assert_eq!(reg.resolve("academicOps").as_deref(), Some("aops"));
        assert_eq!(reg.resolve("ACAOPS").as_deref(), Some("aops"));
        assert_eq!(reg.resolve("ao").as_deref(), Some("aops"));
        assert_eq!(reg.resolve("sessions").as_deref(), Some("sessions"));
        assert_eq!(reg.resolve("mem").as_deref(), Some("mem"));
        assert_eq!(reg.resolve("nonsense"), None);
    }

    #[test]
    fn slug_field_overrides_map_key() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml(
            dir.path(),
            r#"
projects:
  writing-book:
    slug: book
    aliases: [the-book]
"#,
        );
        let reg = PolecatRegistry::load(dir.path()).unwrap().unwrap();
        // slug: is canonical; map key and aliases still resolve to it.
        assert_eq!(reg.resolve("book").as_deref(), Some("book"));
        assert_eq!(reg.resolve("writing-book").as_deref(), Some("book"));
        assert_eq!(reg.resolve("the-book").as_deref(), Some("book"));
        assert_eq!(reg.known_slugs(), &["book".to_string()]);
    }

    #[test]
    fn missing_file_is_ok_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(PolecatRegistry::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn malformed_file_is_err() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml(dir.path(), "projects: [not, a, map]\n");
        assert!(PolecatRegistry::load(dir.path()).is_err());
    }

    #[test]
    fn dangling_project_alias_target_is_err() {
        // `ao: aopps` (typo) must fail at load, not silently mint a slug.
        let dir = tempfile::tempdir().unwrap();
        write_yaml(
            dir.path(),
            "projects:\n  aops: {}\nproject_aliases:\n  ao: aopps\n",
        );
        let err = PolecatRegistry::load(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("ao: aopps"), "{msg}");
        assert!(msg.contains("does not exist"), "{msg}");
    }

    #[test]
    fn builtins_pass_without_registry() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resolve_project(dir.path(), "task").unwrap(), "task");
        assert_eq!(
            resolve_project(dir.path(), "Adhoc-Sessions").unwrap(),
            "adhoc-sessions"
        );
    }

    #[test]
    fn non_builtin_without_registry_hard_fails() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_project(dir.path(), "aops").unwrap_err();
        assert!(err.to_string().contains("no polecat.yaml found"), "{err}");
    }

    #[test]
    fn unregistered_value_lists_known_slugs() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml(dir.path(), CANONICAL);
        let err = resolve_project(dir.path(), "aopps").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown project 'aopps'"), "{msg}");
        assert!(msg.contains("aops"), "{msg}");
    }

    #[test]
    fn resolve_project_canonicalizes_alias() {
        let dir = tempfile::tempdir().unwrap();
        write_yaml(dir.path(), CANONICAL);
        assert_eq!(resolve_project(dir.path(), "ao").unwrap(), "aops");
        assert_eq!(resolve_project(dir.path(), "academicops").unwrap(), "aops");
    }

    #[test]
    fn shared_academicops_file_parses_untouched() {
        // The real academicOps polecat.yaml carries many keys mem ignores.
        let dir = tempfile::tempdir().unwrap();
        write_yaml(
            dir.path(),
            r#"
polecat_home: ~/.polecat
session_defaults:
  hooks_enabled: true
  claude_model: claude-sonnet-4-6
  gemini_model: gemini-3.1-pro-preview
  antigravity_model: agy
  debug: false
  gates:
    handover: block
    qa: warn
    rbg: warn
    hydration: off
    ida: warn
    rbg_review: block
    rbg_threshold: 50
default: {}
crew_defaults: {}
run_defaults: {}
docker:
  image: ghcr.io/nicsuzor/aops-crew
container_env_forward:
  - CLAUDE_CODE_OAUTH_TOKEN
external_agents:
  github:
    enabled: true
    workflows: [agent-enforcer]
projects:
  aops:
    repo: academicOps
    default_branch: main
    aliases: [academicOps, acaops]
  sessions:
    repo: sessions
    default_branch: main
  junior:
    repo: junior
project_aliases:
  ao: aops
crew_names:
  - crew
"#,
        );
        let reg = PolecatRegistry::load(dir.path()).unwrap().unwrap();
        assert_eq!(reg.resolve("junior").as_deref(), Some("junior"));
        assert_eq!(reg.resolve("ao").as_deref(), Some("aops"));
        assert_eq!(
            reg.known_slugs(),
            &["aops".to_string(), "junior".to_string(), "sessions".to_string()]
        );
    }
}
