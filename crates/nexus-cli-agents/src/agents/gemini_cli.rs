//! `GeminiCliAgent` — `CliAgent` implementation for Google's Gemini CLI
//! (binary: `gemini`).
//!
//! Facts below come from a web-research pass against official Gemini CLI
//! documentation (research completed 2026-10-21).
//!
//! CONFIRMED:
//! - Non-interactive single-turn invocation: `-p` / `--prompt "<text>"`.
//! - JSON output: `--output-format json` (alias `-o json`); `stream-json`
//!   is also documented.
//!
//! CAVEAT (documented, not a reason to withhold the feature):
//! - A public GitHub issue (google-gemini/gemini-cli#9009) reports that
//!   the documented JSON output mode is sometimes unreliable in
//!   practice. The flag is still officially documented, so this agent
//!   still builds it when `want_json` is requested and still reports
//!   `has_verified_noninteractive_mode: true` - the caveat is surfaced
//!   via [`RELIABILITY_NOTE`] for the UI layer to show later, not by
//!   withholding the capability.
//!
//! UNVERIFIED (left unset rather than guessed):
//! - A `--version` flag was NOT confirmed against official docs in this
//!   pass, so `detect()` never attempts a version probe; `version` is
//!   always `None`.
//! - No documented login-status query command was found, so `detect()`
//!   always reports `LoginStatus::Unknown`.

use std::path::Path;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{
    CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus,
};

const BINARY_NAMES: &[&str] = &["gemini"];
// No version flag confirmed against official docs - intentionally empty.
const VERSION_ARGS: &[&str] = &[];

/// Caveat for the UI layer: Gemini CLI's JSON output mode is documented,
/// but a public GitHub issue (google-gemini/gemini-cli#9009) reports it
/// is sometimes unreliable in practice. Surfaced here so a future UI can
/// show a hint without this crate guessing at UI copy.
pub const RELIABILITY_NOTE: &str = "Gemini CLI's --output-format json mode is documented, but has been \
     reported unreliable in some versions (see google-gemini/gemini-cli \
     issue #9009). Falling back to plain-text output may be necessary.";

/// Google Gemini CLI (`gemini`) non-interactive CLI driver.
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiCliAgent;

impl GeminiCliAgent {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CliAgent for GeminiCliAgent {
    fn info(&self) -> CliAgentInfo {
        CliAgentInfo {
            id: "gemini-cli",
            display_name: "Gemini CLI",
            binary_names: BINARY_NAMES,
            version_args: VERSION_ARGS,
            homepage: "https://github.com/google-gemini/gemini-cli",
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
        let result = GeminiCliAgent::new().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed_no_version_or_login_probe() {
        let runner = FakeCommandRunner::new().with_which("gemini", "/usr/bin/gemini");
        let result = GeminiCliAgent::new().detect(&runner).await;
        assert!(result.installed);
        assert!(result.version.is_none());
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[test]
    fn build_command_plain_text() {
        let agent = GeminiCliAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "gemini");
        assert_eq!(invocation.args, vec!["-p".to_string(), "hi".to_string()]);
    }

    #[test]
    fn build_command_json() {
        let agent = GeminiCliAgent::new();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hi", &cwd, true)
            .unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-p".to_string(),
                "hi".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ]
        );
        assert_eq!(agent.output_format(true), OutputFormat::Json);
        assert!(!RELIABILITY_NOTE.is_empty());
    }
}
