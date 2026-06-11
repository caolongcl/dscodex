//! Role definitions for the `/role` feature.
//!
//! A role reframes the agent's persona and working style (writer, researcher,
//! ...) without touching system-level rules such as tool usage, sandboxing,
//! or approvals: the role text is injected as a dedicated developer message
//! on top of the base instructions rather than replacing them.
//!
//! Roles come from two sources:
//! - built-in presets compiled into the binary (`src/prompts/roles/*.md`)
//! - user-defined markdown files in `$CODEX_HOME/roles/<name>.md`
//!
//! A user file shadows a built-in role with the same name, except for the
//! reserved [`DEFAULT_ROLE_NAME`], which always means "no role instructions"
//! (the stock coding agent).

use std::fs;
use std::path::Path;
use std::path::PathBuf;

/// Reserved name selecting the stock coding-agent behavior. Resolves to a
/// role with an empty spec, which callers treat as "inject nothing".
pub const DEFAULT_ROLE_NAME: &str = "default";

/// Directory under `CODEX_HOME` holding user-defined role files.
pub const ROLES_DIR_NAME: &str = "roles";

const ROLE_WRITER_SPEC: &str = include_str!("prompts/roles/writer.md");
const ROLE_RESEARCHER_SPEC: &str = include_str!("prompts/roles/researcher.md");

/// Maximum length of a description derived from a custom role file.
const MAX_DERIVED_DESCRIPTION_LEN: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleSpec {
    /// Identifier used in config (`role = "..."`) and the `/role` picker.
    /// For custom roles this is the file stem.
    pub name: String,
    /// One-line summary shown in pickers.
    pub description: String,
    /// Prompt text injected as role instructions. Empty for the default role.
    pub spec: String,
    /// True when the role ships with Codex rather than coming from a user file.
    pub builtin: bool,
}

impl RoleSpec {
    /// True for the reserved default role (no role instructions injected).
    pub fn is_default(&self) -> bool {
        self.name == DEFAULT_ROLE_NAME
    }
}

fn default_role() -> RoleSpec {
    RoleSpec {
        name: DEFAULT_ROLE_NAME.to_string(),
        description: "Stock coding agent — no extra role instructions".to_string(),
        spec: String::new(),
        builtin: true,
    }
}

/// Built-in roles, default first. Order is presentation order in pickers.
pub fn builtin_roles() -> Vec<RoleSpec> {
    vec![
        default_role(),
        RoleSpec {
            name: "writer".to_string(),
            description: "Literary writing partner for poetry, fiction, essays, and articles"
                .to_string(),
            spec: ROLE_WRITER_SPEC.to_string(),
            builtin: true,
        },
        RoleSpec {
            name: "researcher".to_string(),
            description: "Research collaborator for scientific and mathematical work".to_string(),
            spec: ROLE_RESEARCHER_SPEC.to_string(),
            builtin: true,
        },
    ]
}

/// Directory holding user-defined role files for the given `CODEX_HOME`.
pub fn custom_roles_dir(codex_home: &Path) -> PathBuf {
    codex_home.join(ROLES_DIR_NAME)
}

/// Derive a one-line picker description from a custom role file: the first
/// markdown heading if present, otherwise the first non-empty line.
fn derive_description(spec: &str) -> String {
    let line = spec
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Custom role");
    let line = line.trim_start_matches('#').trim();
    let line = if line.is_empty() { "Custom role" } else { line };
    let mut description: String = line.chars().take(MAX_DERIVED_DESCRIPTION_LEN).collect();
    if description.len() < line.len() {
        description.push('…');
    }
    description
}

fn load_custom_role(path: &Path) -> Option<RoleSpec> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return None;
    }
    let name = path.file_stem()?.to_str()?.to_string();
    // The reserved default name cannot be shadowed by a user file.
    if name.is_empty() || name == DEFAULT_ROLE_NAME {
        return None;
    }
    let spec = fs::read_to_string(path).ok()?;
    if spec.trim().is_empty() {
        return None;
    }
    Some(RoleSpec {
        description: derive_description(&spec),
        name,
        spec,
        builtin: false,
    })
}

fn custom_roles(codex_home: &Path) -> Vec<RoleSpec> {
    let Ok(entries) = fs::read_dir(custom_roles_dir(codex_home)) else {
        return Vec::new();
    };
    let mut roles: Vec<RoleSpec> = entries
        .flatten()
        .filter_map(|entry| load_custom_role(&entry.path()))
        .collect();
    roles.sort_by(|a, b| a.name.cmp(&b.name));
    roles
}

/// All available roles: built-ins (default first) followed by custom roles
/// sorted by name. A custom role replaces the built-in with the same name in
/// place, so customized presets keep their position in pickers.
pub fn list_roles(codex_home: &Path) -> Vec<RoleSpec> {
    let mut roles = builtin_roles();
    for custom in custom_roles(codex_home) {
        match roles.iter_mut().find(|role| role.name == custom.name) {
            Some(existing) => *existing = custom,
            None => roles.push(custom),
        }
    }
    roles
}

/// Resolve a role by name: custom file first (so users can shadow presets),
/// then built-ins. Returns `None` for unknown names.
pub fn find_role(codex_home: &Path, name: &str) -> Option<RoleSpec> {
    if name == DEFAULT_ROLE_NAME {
        return Some(default_role());
    }
    let custom_path = custom_roles_dir(codex_home).join(format!("{name}.md"));
    if let Some(role) = load_custom_role(&custom_path) {
        return Some(role);
    }
    builtin_roles().into_iter().find(|role| role.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn builtin_roles_start_with_default_and_have_specs() {
        let roles = builtin_roles();
        assert_eq!(roles[0].name, DEFAULT_ROLE_NAME);
        assert!(roles[0].spec.is_empty());
        for role in &roles[1..] {
            assert!(!role.spec.trim().is_empty(), "{} has empty spec", role.name);
        }
    }

    #[test]
    fn find_role_resolves_builtins_and_rejects_unknown() {
        let home = tempfile::tempdir().unwrap();
        let writer = find_role(home.path(), "writer").unwrap();
        assert!(writer.builtin);
        assert!(writer.spec.contains("Role: Writer"));
        assert!(find_role(home.path(), "no-such-role").is_none());
        assert!(
            find_role(home.path(), DEFAULT_ROLE_NAME)
                .unwrap()
                .is_default()
        );
    }

    #[test]
    fn custom_role_is_listed_and_shadows_builtin() {
        let home = tempfile::tempdir().unwrap();
        let roles_dir = custom_roles_dir(home.path());
        fs::create_dir_all(&roles_dir).unwrap();
        fs::write(
            roles_dir.join("poet.md"),
            "# Tang dynasty poet\nWrite verse.",
        )
        .unwrap();
        fs::write(roles_dir.join("writer.md"), "# My writer\nCustom writer.").unwrap();
        fs::write(
            roles_dir.join("default.md"),
            "# Ignored\nCannot shadow default.",
        )
        .unwrap();
        fs::write(roles_dir.join("empty.md"), "  \n").unwrap();
        fs::write(roles_dir.join("notes.txt"), "not a role").unwrap();

        let roles = list_roles(home.path());
        let names: Vec<&str> = roles.iter().map(|role| role.name.as_str()).collect();
        assert_eq!(
            names,
            vec![DEFAULT_ROLE_NAME, "writer", "researcher", "poet"]
        );

        let writer = find_role(home.path(), "writer").unwrap();
        assert!(!writer.builtin);
        assert_eq!(writer.description, "My writer");

        let poet = find_role(home.path(), "poet").unwrap();
        assert_eq!(poet.description, "Tang dynasty poet");
        assert!(
            find_role(home.path(), DEFAULT_ROLE_NAME)
                .unwrap()
                .spec
                .is_empty()
        );
    }

    #[test]
    fn derive_description_truncates_and_falls_back() {
        assert_eq!(derive_description("# Heading\nbody"), "Heading");
        assert_eq!(
            derive_description("plain first line\nmore"),
            "plain first line"
        );
        assert_eq!(derive_description("   \n\n"), "Custom role");
        let long = "x".repeat(200);
        assert!(derive_description(&long).ends_with('…'));
    }
}
