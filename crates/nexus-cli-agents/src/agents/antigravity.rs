//! `AntigravityAgent` — `CliAgent` implementation for Google Antigravity's
//! CLI (binary: `agy`).
//!
//! Facts below come from a web-research pass against official Antigravity
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Non-interactive single-turn invocation: `-p` / `--print` /
//!   `--prompt "<text>"` (all three forms documented); this agent uses
//!   `-p` as the canonical short form.
//! - `-c` / `--continue` and `--agent <name>` exist as separate
//!   documented flags but are not needed for a single non-interactive
//!   prompt, so they are not included by default.
//!
//! UNVERIFIED (deliberately NOT added):
//! - No JSON output flag was found in official docs.
//!   `build_noninteractive_command` therefore never adds a JSON flag
//!   (ignores `want_json`), and `output_format` always reports
//!   `OutputFormat::PlainText`. This mirrors the Qwen Code treatment
//!   (unconfirmed-absent, not confirmed-absent): `want_json: true` does
//!   NOT produce an error, it just has no observable effect on the
//!   built command.
//! - No `--version` flag was confirmed, so `detect()` never attempts a
//!   version probe; `version` is always `None`.
//! - No documented login-status query command was found, so `detect()`
//!   always reports `LoginStatus::Unknown`.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus};

const BINARY_NAMES: &[&str] = &["agy"];
// No version flag confirmed against official docs - intentionally empty.
const VERSION_ARGS: &[&str] = &[];

/// Google Antigravity (`agy`) non-interactive CLI driver.
#[derive(Debug, Default, Clone, Copy)]
pub struct AntigravityAgent;

impl AntigravityAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CliAgent for AntigravityAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "antigravity",
            display_name: "Google Antigravity",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://antigravity.google/",
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
        _want_json: bool,
    ) -> Result<CliInvocation, UnsupportedMode> {
        // `_want_json` intentionally ignored - see module docs: no JSON
        // flag is confirmed to exist, so we never guess one.
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
        let result = AntigravityAgent::new().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed() {
        let runner = FakeCommandRunner::new().with_which("agy", "/usr/bin/agy");
        let result = AntigravityAgent::new().detect(&runner).await;
        assert!(result.installed);
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[test]
    fn build_command_ignores_want_json() {
        let agent = AntigravityAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let plain = agent
            .build_noninteractive_command("hi", &cwd, false)
            .unwrap();
        let json = agent
            .build_noninteractive_command("hi", &cwd, true)
            .unwrap();
        assert_eq!(plain.args, vec!["-p".to_string(), "hi".to_string()]);
        assert_eq!(json.args, plain.args);
        assert_eq!(agent.output_format(true), OutputFormat::PlainText);
    }
}
