//! `AiderAgent` — `CliAgent` implementation for Aider (binary: `aider`).
//!
//! Facts below come from a web-research pass against official Aider
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Non-interactive single-turn invocation: `--message "<text>"` (alias
//!   `-m`). A `--message-file <path>` form also exists for longer
//!   prompts; not used by default here, just a documented capability.
//! - `--yes-always` skips interactive confirmations. Per Nexus's
//!   auto-apply gating convention (mirrored for Cursor and Copilot CLI),
//!   this is NOT included by default - it is gated behind the
//!   `allow_auto_apply` constructor flag (default `false`).
//! - JSON output is CONFIRMED NOT SUPPORTED - Aider has no JSON output
//!   flag at all. Unlike the "unverified" agents in this crate (Qwen,
//!   Antigravity), this is a confirmed absence, so
//!   `build_noninteractive_command` explicitly returns
//!   `Err(UnsupportedMode)` whenever `want_json` is `true`, rather than
//!   silently ignoring the request. `output_format` always reports
//!   `OutputFormat::PlainText`.
//!
//! UNVERIFIED (left unset rather than guessed):
//! - No `--version` flag was confirmed in this research pass, so
//!   `detect()` never attempts a version probe; `version` is always
//!   `None`.
//! - No documented login-status query command was found (Aider typically
//!   uses API keys via environment variables / config, not an
//!   interactive login flow), so `detect()` always reports
//!   `LoginStatus::Unknown`.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus};

const BINARY_NAMES: &[&str] = &["aider"];
// No version flag confirmed against official docs - intentionally empty.
const VERSION_ARGS: &[&str] = &[];

/// Aider (`aider`) non-interactive CLI driver.
#[derive(Debug, Clone, Copy)]
pub struct AiderAgent {
    /// When `true`, `build_noninteractive_command` adds `--yes-always`
    /// so Aider skips its own interactive confirmation prompts. Defaults
    /// to `false` - Nexus's own confirm-before-file-touching-action UX
    /// should gate this instead unless the caller explicitly opts in.
    pub allow_auto_apply: bool,
}

impl AiderAgent {
    pub fn new(allow_auto_apply: bool) -> Self {
        Self { allow_auto_apply }
    }
}

impl Default for AiderAgent {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl CliAgent for AiderAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "aider",
            display_name: "Aider",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://aider.chat/",
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
        if want_json {
            return Err(UnsupportedMode(
                "Aider has no JSON output mode (confirmed absent in official \
                 docs); only plain-text non-interactive output via --message \
                 is supported."
                    .to_string(),
            ));
        }
        let mut args = vec!["--message".to_string(), prompt.to_string()];
        if self.allow_auto_apply {
            args.push("--yes-always".to_string());
        }
        Ok(CliInvocation {
            program: BINARY_NAMES[0].to_string(),
            args,
            cwd: Some(cwd.to_path_buf()),
        })
    }

    fn output_format(&self, _want_json: bool) -> OutputFormat {
        OutputFormat::PlainText
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
        let result = AiderAgent::default().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed() {
        let runner = FakeCommandRunner::new().with_which("aider", "/usr/bin/aider");
        let result = AiderAgent::default().detect(&runner).await;
        assert!(result.installed);
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[test]
    fn build_command_plain_text_default() {
        let agent = AiderAgent::default();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("fix bug", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "aider");
        assert_eq!(
            invocation.args,
            vec!["--message".to_string(), "fix bug".to_string()]
        );
    }

    #[test]
    fn build_command_with_auto_apply() {
        let agent = AiderAgent::new(true);
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("fix bug", &cwd, false)
            .unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "--message".to_string(),
                "fix bug".to_string(),
                "--yes-always".to_string(),
            ]
        );
    }

    #[test]
    fn build_command_json_is_explicitly_unsupported() {
        let agent = AiderAgent::default();
        let cwd = PathBuf::from("/tmp/project");
        let err = agent
            .build_noninteractive_command("fix bug", &cwd, true)
            .unwrap_err();
        assert!(err.0.to_lowercase().contains("json"));
        assert_eq!(agent.output_format(true), OutputFormat::PlainText);
    }
}
