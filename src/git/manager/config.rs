//! Configuration for Git operations.

/// Configuration for Git operations
#[derive(Debug, Clone)]
pub struct GitConfig {
    /// Default remote name for push operations
    pub default_remote: String,
    /// Whether to create annotated tags
    pub annotated_tags: bool,
    /// Whether to push tags automatically
    pub auto_push_tags: bool,
    /// Custom commit message template
    pub commit_message_template: Option<String>,
    /// Custom tag message template
    pub tag_message_template: Option<String>,
    /// Whether to verify signatures
    pub verify_signatures: bool,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            default_remote: "origin".to_string(),
            annotated_tags: true,
            auto_push_tags: true,
            commit_message_template: None,
            tag_message_template: None,
            verify_signatures: false,
        }
    }
}

impl GitConfig {
    /// Generate commit message for release
    pub fn generate_commit_message(&self, version: &semver::Version) -> String {
        if let Some(ref template) = self.commit_message_template {
            template.replace("{version}", &version.to_string())
        } else {
            format!("release: v{}", version)
        }
    }

    /// Generate tag message for release
    pub fn generate_tag_message(&self, version: &semver::Version) -> String {
        if let Some(ref template) = self.tag_message_template {
            template.replace("{version}", &version.to_string())
        } else {
            format!("Release v{}", version)
        }
    }
}
