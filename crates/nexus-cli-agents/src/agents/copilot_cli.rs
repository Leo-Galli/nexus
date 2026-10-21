//! `CopilotCliAgent` — `CliAgent` implementation for the standalone GitHub
//! Copilot CLI (binary: `copilot`).
//!
//! NOTE: The older `gh copilot` `gh` extension is retired/deprecated and
//! is deliberately NOT implemented here - only the standalone `copilot`
//! binary is supported.
//!
//! Facts below come from a web-research pass against official GitHub
//! Copilot CLI documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Non-interactive single-turn invocation: `-p` / `--prompt "<text>"`.
//! - `--allow-all-tools` is documented as required for a fully unattended
//!   run. Per Nexus's auto-apply gating convention (mirrored for Cursor
//!   and Aider), this is NOT included by default - it is gated behind
//!   the `allow_auto_apply` constructor flag (default `false`). Without
//!   it, the CLI may still prompt interactively, which is a known
//!   limitation of driving this tool non-interactively from a GUI; Nexus
//!   should prefer the VTE terminal integration for this agent when
//!   `allow_auto_apply` is `false` (a UI-layer decision, documented here
//!   only).
//! - JSON output: `--output-format=json`, producing JSONL (one JSON
//!   object per line). As of v1.0.97, secrets are documented as redacted
//!   in this output. Parsing the JSONL stream itself is a UI-layer
//!   concern outside this crate's scope - this module only builds the
//!   `CliInvocation`.
//!
//! UNVERIFIED (left unset rather than guessed):
//! - No `--version` flag was confirmed in this research pass, so
//!   `detect()` never attempts a version probe; `version` is always
//!   `None`.
//! - No documented login-status query command was found for the
//!   standalone `copilot` binary, so `detect()` always reports
//!   `LoginStatus::Unknown`.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus};

const BINARY_NAMES: &[&str] = &["copilot"];
// No version flag confirmed against official docs - intentionally empty.
const VERSION_ARGS: &[&str] = &[];

/// GitHub Copilot CLI (`copilot`, standalone binary) non-interactive
/// driver.
#[derive(Debug, Clone, Copy)]
pub struct CopilotCliAgent {
    /// When `true`, `build_noninteractive_command` adds
    /// `--allow-all-tools`, which is documented as required for a fully
    /// unattended run. Defaults to `false`; without it the CLI may still
    /// prompt interactively (a known limitation of driving this tool
    /// non-interactively) - Nexus should prefer the VTE terminal
    /// integration for this agent in that case.
    pub allow_auto_apply: bool,
}

impl CopilotCliAgent {
    pub fn new(allow_auto_apply: bool) -> Self {
        Self { allow_auto_apply }
    }
}

impl Default for CopilotCliAgent {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl CliAgent for CopilotCliAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "copilot-cli",
            display_name: "GitHub Copilot CLI",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli",
            has_verified_noninteractive_mode: true,
        }
    }

    async fn detect(&self, runner: &dyn CommandRunner) -> CliDetectionResult {
        let mut binary_path = None;
        for name in BINARY_NAMES {
            if let Some(path) = runner.which(name).await {
                binary_path = Some(path);
                break;
            }
        }
        let Some(binary_path) = binary_path else {
            return CliDetectionResult::not_installed();
        };

        CliDetectionResult {
            installed: true,
            binary_path: Some(binary_path.display().to_string()),
            version: None,
            login_status: LoginStatus::Unknown,
        }
    }

    fn build_noninteractive_command(
        &self,
        prompt: &str,
        cwd: &Path,
        want_json: bool,
    ) -> Result<CliInvocation, UnsupportedMode> {
        let mut args = vec!["-p".to_string(), prompt.to_string()];
        if self.allow_auto_apply {
            args.push("--allow-all-tools".to_string());
        }
        if want_json {
            args.push("--output-format=json".to_string());
        }
        Ok(CliInvocation {
            program: BINARY_NAMES[0].to_string(),
            args,
            cwd: Some(cwd.to_path_buf()),
        })
    }

    fn output_format(&self, want_json: bool) -> OutputFormat {
        if want_json {
            OutputFormat::Json
        } else {
            OutputFormat::PlainText
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeCommandRunner;
    use std::path::PathBuf;

    #[tokio::test]
    async fn detect_not_installed() {
        let runner = FakeCommandRunner::new();
        let result = CopilotCliAgent::default().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed() {
        let runner = FakeCommandRunner::new().with_which("copilot", "/usr/bin/copilot");
        let result = CopilotCliAgent::default().detect(&runner).await;
        assert!(result.installed);
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[test]
    fn build_command_default_no_allow_all_tools() {
        let agent = CopilotCliAgent::default();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "copilot");
        assert_eq!(invocation.args, vec!["-p".to_string(), "hi".to_string()]);
    }

    #[test]
    fn build_command_auto_apply_and_json() {
        let agent = CopilotCliAgent::new(true);
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, true)
            .unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-p".to_string(),
                "hi".to_string(),
                "--allow-all-tools".to_string(),
                "--output-format=json".to_string(),
            ]
        );
        assert_eq!(agent.output_format(true), OutputFormat::Json);
    }
}
