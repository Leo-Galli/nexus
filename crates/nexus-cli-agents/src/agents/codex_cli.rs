//! `CodexCliAgent` — `CliAgent` implementation for OpenAI's Codex CLI
//! (binary: `codex`).
//!
//! Facts below come from a web-research pass against official Codex CLI
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Non-interactive single-turn invocation: `codex exec "<prompt>"`
//!   (documented alias: `codex e`; this implementation uses the canonical
//!   `exec` subcommand).
//! - JSON output: `codex exec --json "<prompt>"`, which streams JSONL
//!   (one JSON object per line) to stdout.
//! - `-o` / `--output-last-message <file>` captures the final message to
//!   a file. This is NOT used by `build_noninteractive_command` by
//!   default (it changes where output lands rather than being required
//!   for non-interactive operation) - documented here only as a known
//!   capability for a future caller that wants to request it explicitly.
//! - Login status can be queried with `codex login status` (and
//!   `codex whoami`); signing in is done via the interactive `codex
//!   login` command. Nexus does not auto-invoke `codex login` - that is
//!   strictly a user-initiated action from a "Log in" button in the UI.
//!
//! UNVERIFIED (left unset rather than guessed):
//! - A `--version` flag was NOT confirmed against official docs in this
//!   pass, so `detect()` never attempts a version probe; `version` is
//!   always `None`.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{
    CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus,
};

const BINARY_NAMES: &[&str] = &["codex"];
// No version flag confirmed against official docs - intentionally empty.
const VERSION_ARGS: &[&str] = &[];

/// OpenAI Codex CLI (`codex`) non-interactive CLI driver.
#[derive(Debug, Default, Clone, Copy)]
pub struct CodexCliAgent;

impl CodexCliAgent {
    pub fn new() -> Self {
        Self
    }
}

/// Heuristically classify the captured stdout of `codex login status`.
/// Never panics on unexpected output - falls back to `Unknown`.
fn classify_login_status(stdout_lower: &str) -> LoginStatus {
    if stdout_lower.contains("not logged in") || stdout_lower.contains("not authenticated") {
        LoginStatus::LoggedOut
    } else if stdout_lower.contains("logged in") || stdout_lower.contains("authenticated") {
        LoginStatus::LoggedIn
    } else {
        LoginStatus::Unknown
    }
}

#[async_trait]
impl CliAgent for CodexCliAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "codex-cli",
            display_name: "OpenAI Codex CLI",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://developers.openai.com/codex/cli",
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

        // Detection probe: `codex login status`. Any run error or
        // non-zero exit is treated as Unknown, never a crash.
        let login_args = vec!["login".to_string(), "status".to_string()];
        let login_status = match runner.run(BINARY_NAMES[0], &login_args, None).await {
            Ok(output) if output.success() => {
                classify_login_status(&output.stdout_utf8_lossy().to_lowercase())
            }
            Ok(_) => LoginStatus::Unknown,
            Err(_) => LoginStatus::Unknown,
        };

        CliDetectionResult {
            installed: true,
            binary_path: Some(binary_path.display().to_string()),
            version: None,
            login_status,
        }
    }

    fn build_noninteractive_command(
        &self,
        prompt: &str,
        cwd: &Path,
        want_json: bool,
    ) -> Result<CliInvocation, UnsupportedMode> {
        let mut args = vec!["exec".to_string()];
        if want_json {
            args.push("--json".to_string());
        }
        args.push(prompt.to_string());
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
        let result = CodexCliAgent::new().detect(&runner).await;
        assert!(!result.installed);
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[tokio::test]
    async fn detect_reports_logged_in_from_status_probe() {
        let runner = FakeCommandRunner::new()
            .with_which("codex", "/usr/bin/codex")
            .with_run_result(
                "codex",
                &["login", "status"],
                "Logged in using ChatGPT account\n",
                "",
                0,
            );
        let result = CodexCliAgent::new().detect(&runner).await;
        assert!(result.installed);
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::LoggedIn);
    }

    #[tokio::test]
    async fn detect_reports_logged_out_from_status_probe() {
        let runner = FakeCommandRunner::new()
            .with_which("codex", "/usr/bin/codex")
            .with_run_result("codex", &["login", "status"], "Not logged in\n", "", 0);
        let result = CodexCliAgent::new().detect(&runner).await;
        assert_eq!(result.login_status, LoginStatus::LoggedOut);
    }

    #[tokio::test]
    async fn detect_reports_unknown_when_probe_errors_or_fails() {
        let runner = FakeCommandRunner::new().with_which("codex", "/usr/bin/codex");
        // No run_result programmed -> FakeCommandRunner::run returns Err.
        let result = CodexCliAgent::new().detect(&runner).await;
        assert_eq!(result.login_status, LoginStatus::Unknown);

        let runner_nonzero = FakeCommandRunner::new()
            .with_which("codex", "/usr/bin/codex")
            .with_run_result("codex", &["login", "status"], "", "boom", 1);
        let result = CodexCliAgent::new().detect(&runner_nonzero).await;
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[test]
    fn build_command_plain_text() {
        let agent = CodexCliAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("do the thing", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "codex");
        assert_eq!(
            invocation.args,
            vec!["exec".to_string(), "do the thing".to_string()]
        );
    }

    #[test]
    fn build_command_json() {
        let agent = CodexCliAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("do the thing", &cwd, true)
            .unwrap();
        assert_eq!(invocation.program, "codex");
        assert_eq!(
            invocation.args,
            vec![
                "exec".to_string(),
                "--json".to_string(),
                "do the thing".to_string(),
            ]
        );
        assert_eq!(agent.output_format(true), OutputFormat::Json);
    }
}
