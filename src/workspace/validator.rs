//! Workspace validation for release checks.
#![allow(dead_code)]

use crate::error::Result;
use crate::workspace::SharedWorkspaceInfo;
use serde::{Deserialize, Serialize};

/// Workspace validator
#[derive(Debug)]
pub struct WorkspaceValidator {
    workspace: SharedWorkspaceInfo,
}

/// Validation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub success: bool,
    pub checks: Vec<ValidationCheck>,
    pub critical_errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Individual validation check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
    pub critical: bool,
    pub duration_ms: u64,
}

impl WorkspaceValidator {
    /// Create a new workspace validator
    pub fn new(workspace: SharedWorkspaceInfo) -> Self {
        Self { workspace }
    }

    /// Perform workspace validation
    pub async fn validate(&self) -> Result<ValidationResult> {
        let mut checks = Vec::new();
        let mut critical_errors = Vec::new();
        let mut warnings = Vec::new();

        // Version consistency validation
        self.validate_version_consistency(&mut checks, &mut critical_errors, &mut warnings);

        let success = critical_errors.is_empty();

        Ok(ValidationResult {
            success,
            checks,
            critical_errors,
            warnings,
        })
    }

    /// Validate version consistency across packages
    fn validate_version_consistency(
        &self,
        checks: &mut Vec<ValidationCheck>,
        critical_errors: &mut Vec<String>,
        warnings: &mut Vec<String>,
    ) {
        let start_time = std::time::Instant::now();

        let workspace_version = match self.workspace.workspace_version() {
            Ok(version) => version,
            Err(e) => {
                let error_msg = format!("Failed to get workspace version: {}", e);
                checks.push(ValidationCheck {
                    name: "Version Consistency".to_string(),
                    passed: false,
                    message: error_msg.clone(),
                    critical: true,
                    duration_ms: start_time.elapsed().as_millis() as u64,
                });
                critical_errors.push(error_msg);
                return;
            }
        };

        let mut version_mismatches = Vec::new();

        for (package_name, package_info) in &self.workspace.packages {
            if package_info.version != workspace_version {
                let mismatch = format!(
                    "Package '{}' version '{}' doesn't match workspace version '{}'",
                    package_name, package_info.version, workspace_version
                );
                version_mismatches.push(mismatch);
            }
        }

        let duration = start_time.elapsed().as_millis() as u64;

        if version_mismatches.is_empty() {
            checks.push(ValidationCheck {
                name: "Version Consistency".to_string(),
                passed: true,
                message: format!(
                    "All packages consistent with workspace version {}",
                    workspace_version
                ),
                critical: true,
                duration_ms: duration,
            });
        } else {
            checks.push(ValidationCheck {
                name: "Version Consistency".to_string(),
                passed: false,
                message: version_mismatches.join("; "),
                critical: true,
                duration_ms: duration,
            });
            critical_errors.extend(version_mismatches);
        }

        let _ = warnings; // Suppress unused warning
    }
}

impl ValidationResult {
    pub fn failed_checks(&self) -> Vec<&ValidationCheck> {
        self.checks.iter().filter(|check| !check.passed).collect()
    }

    pub fn summary(&self) -> String {
        let total_checks = self.checks.len();
        let passed_checks = self.checks.iter().filter(|c| c.passed).count();

        if self.success {
            format!("✅ All {} checks passed", total_checks)
        } else {
            format!("❌ {}/{} checks passed", passed_checks, total_checks)
        }
    }
}
