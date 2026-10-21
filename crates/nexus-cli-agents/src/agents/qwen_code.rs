//! `QwenCodeAgent` — `CliAgent` implementation for Alibaba's Qwen Code CLI
//! (binary: `qwen`).
//!
//! Facts below come from a web-research pass against official Qwen Code
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Version query: `qwen --version`.
//! - Non-interactive single-turn invocation: `-p "<prompt>"`.
//!
//! UNVERIFIED (deliberately NOT added):
//! - No JSON output flag was found in official docs. Because this is
//!   unconfirmed (not confirmed-absent, just not found),
//!   `build_noninteractive_command` does NOT error when `want_json` is
//!   `true` - it simply never adds a (possibly nonexistent) JSON flag,
//!   and `output_format` always reports `OutputFormat::PlainText`
//!   regardless of `want_json`. This differs from Aider, where the
//!   absence of a JSON mode is confirmed and callers get an explicit
//!   `Err(UnsupportedMode)` instead.
//! - No documented login-status query command was found, so `detect()`
//!   always reports `LoginStatus::Unknown`.
//!
//! UI GUIDANCE NOTE (no code behaviour attached):
//! - Research indicates headless-mode auth for Qwen Code is
//!   environment-variable only (no browser-based OAuth fallback in
//!   non-interactive/headless contexts). A future Preferences/login UI
//!   for this agent should guide the user to set the documented
//!   environment variable(s) rather than expecting an interactive
//!   browser login flow to work headlessly.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus};

const BINARY_NAMES: &[&str] = &["qwen"];
const VERSION_ARGS: &[&str] = &["--version"];

/// Defensively extract a plausible version substring from `--version`
/// output. Never panics; falls back to the first non-empty line, or
/// `None` if the output is empty/unparseable.
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

/// Qwen Code (`qwen`) non-interactive CLI driver.
#[derive(Debug, Default, Clone, Copy)]
pub struct QwenCodeAgent;

impl QwenCodeAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CliAgent for QwenCodeAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "qwen-code",
            display_name: "Qwen Code",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://github.com/QwenLM/qwen-code",
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

        let version_args: Vec<String> = VERSION_ARGS.iter().map(|s| s.to_string()).collect();
        let version = match runner.run(BINARY_NAMES[0], &version_args, None).await {
            Ok(output) => parse_version(&output.stdout_utf8_lossy()),
            Err(_) => None,
        };

        CliDetectionResult {
            installed: true,
            binary_path: Some(binary_path.display().to_string()),
            version,
            login_status: LoginStatus::Unknown,
        }
    }

    fn build_noninteractive_command(
        &self,
        prompt: &str,
        cwd: &Path,
        _want_json: bool,
    ) -> Result<CliInvocation, UnsupportedMode> {
        // `_want_json` is intentionally ignored - see module docs: no
        // JSON flag is confirmed to exist, so we never guess one.
        let args = vec!["-p".to_string(), prompt.to_string()];
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
        let result = QwenCodeAgent::new().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed_parses_version() {
        let runner = FakeCommandRunner::new()
            .with_which("qwen", "/usr/bin/qwen")
            .with_run_result("qwen", &["--version"], "qwen-code 0.4.2\n", "", 0);
        let result = QwenCodeAgent::new().detect(&runner).await;
        assert!(result.installed);
        assert_eq!(result.version.as_deref(), Some("0.4.2"));
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[tokio::test]
    async fn detect_installed_version_probe_failure_is_not_fatal() {
        let runner = FakeCommandRunner::new().with_which("qwen", "/usr/bin/qwen");
        let result = QwenCodeAgent::new().detect(&runner).await;
        assert!(result.installed);
        assert!(result.version.is_none());
    }

    #[test]
    fn build_command_ignores_want_json_false() {
        let agent = QwenCodeAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "qwen");
        assert_eq!(invocation.args, vec!["-p".to_string(), "hi".to_string()]);
        assert_eq!(agent.output_format(false), OutputFormat::PlainText);
    }

    #[test]
    fn build_command_ignores_want_json_true_without_error() {
        let agent = QwenCodeAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        // Must NOT error, and must NOT add any JSON-ish flag.
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, true)
            .unwrap();
        assert_eq!(invocation.args, vec!["-p".to_string(), "hi".to_string()]);
        assert_eq!(agent.output_format(true), OutputFormat::PlainText);
    }
}
