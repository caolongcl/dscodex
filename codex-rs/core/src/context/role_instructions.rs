use super::ContextualUserFragment;

pub(crate) const ROLE_SPEC_OPEN_TAG: &str = "<role_spec>";
pub(crate) const ROLE_SPEC_CLOSE_TAG: &str = "</role_spec>";

/// Injects the active role's persona text as a developer message. The role
/// reframes persona, priorities, and presentation; system-level rules (tool
/// usage, sandboxing, approvals) from the base instructions stay in force.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RoleInstructions {
    spec: String,
}

impl RoleInstructions {
    pub(crate) fn new(spec: impl Into<String>) -> Self {
        Self { spec: spec.into() }
    }
}

impl ContextualUserFragment for RoleInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (ROLE_SPEC_OPEN_TAG, ROLE_SPEC_CLOSE_TAG)
    }

    fn body(&self) -> String {
        format!(
            " The user has assigned you a role for this session. Adopt it fully: it defines your persona, priorities, and how you frame and present your work. The role does not change system-level rules — instructions above about tool usage, sandboxing, approvals, and safety still apply.\n{} ",
            self.spec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_spec_inside_role_markers() {
        let fragment = RoleInstructions::new("# Role: Writer\nWrite well.");
        let body = fragment.body();
        assert!(body.contains("# Role: Writer"));
        assert!(body.contains("assigned you a role"));
        assert!(
            RoleInstructions::type_markers()
                .0
                .starts_with("<role_spec>")
        );
    }
}
