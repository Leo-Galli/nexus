//! `ClaudeCodeAgent` — `CliAgent` implementation for Anthropic's Claude
//! Code CLI (binary: `claude`).
//!
//! Facts below come from a web-research pass against official Claude Code
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Non-interactive single-turn invocation: `-p` / `--print "<prompt>"`.
//! - JSON output: `--output-format json` (or `stream-json`), valid only
//!   together with `-p`/`--print`.
//!
//! DELIBERATELY NOT ADDED:
//! - `--permission-mode <mode>` exists for unattended/scripted runs, but
//!   the exact accepted value strings (e.g. whether `acceptEdits` is
//!   spelled exactly that way in the current release) were not confirmed
//!   with 100% certainty in this research pass. Rather than hardcode a
//!   guessed value, this agent builds the command WITHOUT a
//!   `--permission-mode` flag by default, leaving Claude Code's own
//!   default prompt-confirmation behaviour in effect. Nexus's own
//!   "confirm before file-touching actions" UX layer sits above the CLI
//!   regardless, so this is not a functional gap — just a deliberately
//!   unopinionated default. A future caller that knows a verified
//!   permission-mode value for the installed version can still pass it
//!   through an "extra args" mechanism once one exists.
//!
//! UNVERIFIED (left unset rather than guessed):
//! - A `--version` flag was NOT confirmed against official docs in this
//!   pass, so `detect()` never attempts a version probe for this agent;
//!   `version` is always `None`.
//! - A documented way to query login/auth status (akin to `codex login
//!   status`) was NOT found, so `detect()` always reports
//!   `LoginStatus::Unknown`.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{
    CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus,
};

const BINARY_NAMES: &[&str] = &["claude"];
// No version flag confirmed against official docs - intentionally empty.
const VERSION_ARGS: &[&str] = &[];

/// Claude Code (`claude`) non-interactive CLI driver.
#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeCodeAgent;

impl ClaudeCodeAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CliAgent for ClaudeCodeAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "claude-code",
            display_name: "Claude Code",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://docs.claude.com/en/docs/claude-code",
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

        // Version flag unconfirmed (see module docs) - never probed/guessed.
        // Login status unconfirmed (see module docs) - always Unknown.
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
        if want_json {
            args.push("--output-format".to_string());
            args.push("json".to_string());
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
    async fn detect_reports_not_installed_when_binary_missing() {
        let runner = FakeCommandRunner::new();
        let result = ClaudeCodeAgent::new().detect(&runner).await;
        assert!(!result.installed);
        assert!(result.binary_path.is_none());
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[tokio::test]
    async fn detect_reports_installed_with_no_version_or_login_probe() {
        let runner = FakeCommandRunner::new().with_which("claude", "/usr/bin/claude");
        let result = ClaudeCodeAgent::new().detect(&runner).await;
        assert!(result.installed);
        assert_eq!(result.binary_path.as_deref(), Some("/usr/bin/claude"));
        // Deliberately never probed.
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::Unknown);
        // No run() calls should have been made at all.
        assert!(runner.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn build_command_plain_text() {
        let agent = ClaudeCodeAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hello world", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "claude");
        assert_eq!(
            invocation.args,
            vec!["-p".to_string(), "hello world".to_string()]
        );
        assert_eq!(invocation.cwd, Some(cwd));
    }

    #[test]
    fn build_command_json() {
        let agent = ClaudeCodeAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hello world", &cwd, true)
            .unwrap();
        assert_eq!(invocation.program, "claude");
        assert_eq!(
            invocation.args,
            vec![
                "-p".to_string(),
                "hello world".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ]
        );
        assert_eq!(agent.output_format(true), OutputFormat::Json);
        assert_eq!(agent.output_format(false), OutputFormat::PlainText);
    }
}
