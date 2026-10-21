//! `CursorAgent` — `CliAgent` implementation for Cursor's CLI coding
//! agent.
//!
//! Facts below come from a web-research pass against official Cursor CLI
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - The actual installed binary is `agent`; the installer also creates
//!   a `cursor-agent` symlink. Both names are listed in `binary_names`,
//!   with `agent` first (preferred) per the trait's documented
//!   which()-priority-order contract.
//! - Non-interactive single-turn invocation: `-p` / `--print "<prompt>"`.
//! - `--force` auto-applies changes without interactive confirmation.
//!   Per Nexus's auto-apply gating convention (mirrored for Aider and
//!   Copilot CLI), this is NOT included by default - it is gated behind
//!   the `allow_auto_apply` constructor flag (default `false`), so
//!   Nexus's own confirm-before-file-touching-action UX stays in
//!   control unless the caller explicitly opts in.
//! - JSON output: `--output-format json|text|stream-json`.
//! - Login status: `agent status` is documented to print either
//!   "Not logged in" or version+auth state. `detect()` runs this as a
//!   probe and does a case-insensitive substring match for
//!   "not logged in" to decide `LoggedOut` vs `LoggedIn`; `Unknown` only
//!   if the probe itself fails to run (regardless of exit code,
//!   mirroring the documented behaviour that `agent status` prints its
//!   message either way).
//!
//! BEST EFFORT:
//! - Version: `agent --version` is used as a generic, best-effort
//!   version probe for this class of CLI tool (explicitly sanctioned for
//!   this agent, unlike most others in this crate where version flags
//!   were left unconfirmed). Output is parsed defensively; `version`
//!   gracefully stays `None` if the output doesn't look like a version
//!   string.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus};

const BINARY_NAMES: &[&str] = &["agent", "cursor-agent"];
const VERSION_ARGS: &[&str] = &["--version"];

fn parse_version(stdout: &str) -> Option<String> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(re) = regex::Regex::new(r"\d+\.\d+(?:\.\d+)?")
        && let Some(found) = re.find(trimmed) {
            return Some(found.as_str().to_string());
        }
    trimmed.lines().next().map(|s| s.to_string())
}

/// Cursor CLI (`agent`, a.k.a. `cursor-agent`) non-interactive driver.
#[derive(Debug, Clone, Copy)]
pub struct CursorAgent {
    /// When `true`, `build_noninteractive_command` adds `--force` so
    /// Cursor auto-applies changes without its own interactive
    /// confirmation prompts. Defaults to `false` - Nexus's own
    /// confirm-before-file-touching-action UX should gate this instead
    /// unless the caller explicitly opts in.
    pub allow_auto_apply: bool,
}

impl CursorAgent {
    pub fn new(allow_auto_apply: bool) -> Self {
        Self { allow_auto_apply }
    }
}

impl Default for CursorAgent {
    fn default() -> Self {
        Self::new(false)
    }
}

#[async_trait]
impl CliAgent for CursorAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "cursor-agent",
            display_name: "Cursor CLI",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://docs.cursor.com/en/cli/overview",
            has_verified_noninteractive_mode: true,
        }
    }

    async fn detect(&self, runner: &dyn CommandRunner) -> CliDetectionResult {
        let mut found: Option<(&'static str, std::path::PathBuf)> = None;
        for name in BINARY_NAMES {
            if let Some(path) = runner.which(name).await {
                found = Some((name, path));
                break;
            }
        }
        let Some((binary, binary_path)) = found else {
            return CliDetectionResult::not_installed();
        };

        let version_args: Vec<String> = VERSION_ARGS.iter().map(|s| s.to_string()).collect();
        let version = match runner.run(binary, &version_args, None).await {
            Ok(output) => parse_version(&output.stdout_utf8_lossy()),
            Err(_) => None,
        };

        // `agent status` prints "Not logged in" or version+auth state
        // either way, so we check its stdout regardless of exit code -
        // only a failure to run the probe at all yields Unknown.
        let status_args = vec!["status".to_string()];
        let login_status = match runner.run(binary, &status_args, None).await {
            Ok(output) => {
                let text = output.stdout_utf8_lossy().to_lowercase();
                if text.contains("not logged in") {
                    LoginStatus::LoggedOut
                } else {
                    LoginStatus::LoggedIn
                }
            }
            Err(_) => LoginStatus::Unknown,
        };

        CliDetectionResult {
            installed: true,
            binary_path: Some(binary_path.display().to_string()),
            version,
            login_status,
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
            args.push("--force".to_string());
        }
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
    async fn detect_not_installed() {
        let runner = FakeCommandRunner::new();
        let result = CursorAgent::default().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed_logged_in() {
        let runner = FakeCommandRunner::new()
            .with_which("agent", "/usr/bin/agent")
            .with_run_result("agent", &["--version"], "cursor-agent 1.2.3\n", "", 0)
            .with_run_result("agent", &["status"], "Logged in as dev@example.com\n", "", 0);
        let result = CursorAgent::default().detect(&runner).await;
        assert!(result.installed);
        assert_eq!(result.version.as_deref(), Some("1.2.3"));
        assert_eq!(result.login_status, LoginStatus::LoggedIn);
    }

    #[tokio::test]
    async fn detect_installed_logged_out() {
        let runner = FakeCommandRunner::new()
            .with_which("agent", "/usr/bin/agent")
            .with_run_result("agent", &["--version"], "cursor-agent 1.2.3\n", "", 0)
            .with_run_result("agent", &["status"], "Not logged in\n", "", 1);
        let result = CursorAgent::default().detect(&runner).await;
        assert_eq!(result.login_status, LoginStatus::LoggedOut);
    }

    #[tokio::test]
    async fn detect_status_probe_run_failure_is_unknown() {
        let runner = FakeCommandRunner::new().with_which("agent", "/usr/bin/agent");
        let result = CursorAgent::default().detect(&runner).await;
        assert_eq!(result.login_status, LoginStatus::Unknown);
        assert!(result.version.is_none());
    }

    #[test]
    fn build_command_default_no_force() {
        let agent = CursorAgent::default();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "agent");
        assert_eq!(invocation.args, vec!["-p".to_string(), "hi".to_string()]);
    }

    #[test]
    fn build_command_with_auto_apply_and_json() {
        let agent = CursorAgent::new(true);
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, true)
            .unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-p".to_string(),
                "hi".to_string(),
                "--force".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ]
        );
    }
}
