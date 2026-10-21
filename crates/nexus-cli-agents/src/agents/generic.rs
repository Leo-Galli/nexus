//! `GenericCliAgent` — a fully user-configurable `CliAgent` for any CLI
//! not already modeled by a dedicated struct in this crate.
//!
//! This exists because the research pass found no official, standalone
//! Windsurf agent CLI (only unofficial third-party wrappers), so Windsurf
//! is intentionally NOT given its own module (see
//! `crate::agents::mod` doc comment and `docs/LIMITATIONS.md`). Users can
//! add Windsurf - or any other CLI Nexus doesn't ship a dedicated
//! implementation for - by configuring one of these from Preferences.
//!
//! IMPORTANT SEMANTIC NOTE: unlike every other agent in this crate,
//! `has_verified_noninteractive_mode` here does NOT mean "Nexus's authors
//! verified this against the tool's official documentation". It means
//! "the user who configured this entry asserts it works this way". The
//! field is read directly from the user-supplied `has_noninteractive_support`
//! flag.
//!
//! IMPLEMENTATION NOTE on `CliAgentInfo`'s `'static` string fields: this
//! struct holds owned, runtime-configured `String`s (as it must, since
//! users type these into a settings UI), but `CliAgentInfo` requires
//! `&'static str`/`&'static [&'static str]`. `info()` reconciles this by
//! leaking the configured strings into `'static` memory exactly once per
//! instance (cached in `cached_info`, built lazily on first call) rather
//! than on every call. This is an accepted, bounded, one-time-per-agent
//! leak: `GenericCliAgent` instances are long-lived, user-configured
//! singletons (one per custom CLI the user adds), not created in a hot
//! loop.

use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use nexus_core::cli_agent::{OutputFormat, UnsupportedMode};
use nexus_core::{CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandRunner, LoginStatus};

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

/// A user-configured, generic non-interactive CLI driver.
pub struct GenericCliAgent {
    /// Stable, user-chosen key for this configured agent.
    pub id: String,
    pub display_name: String,
    pub binary_name: String,
    /// Arguments that print a version string, e.g. `["--version"]`.
    /// Empty means "don't probe for a version".
    pub version_args: Vec<String>,
    /// e.g. `Some("-p".into())`. `None` means the CLI takes the prompt
    /// as a bare positional argument with no preceding flag.
    pub prompt_flag: Option<String>,
    /// e.g. `Some("--output-format json".into())` - split on whitespace
    /// when building args. `None` means no known way to request JSON
    /// output; `want_json: true` is then a silent no-op (mirrors the
    /// Qwen/Antigravity "unverified, not confirmed-absent" treatment,
    /// since the user simply may not have configured one yet).
    pub json_flag: Option<String>,
    /// Always appended to the built command, after the prompt and any
    /// JSON flag.
    pub extra_args: Vec<String>,
    /// User-asserted: "I'm telling Nexus this CLI has a working
    /// non-interactive invocation convention, as configured above." See
    /// the module doc comment for why this differs semantically from
    /// every other agent's `has_verified_noninteractive_mode`.
    pub has_noninteractive_support: bool,

    cached_info: OnceLock<CliAgentInfo>,
}

impl GenericCliAgent {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        binary_name: impl Into<String>,
        version_args: Vec<String>,
        prompt_flag: Option<String>,
        json_flag: Option<String>,
        extra_args: Vec<String>,
        has_noninteractive_support: bool,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            binary_name: binary_name.into(),
            version_args,
            prompt_flag,
            json_flag,
            extra_args,
            has_noninteractive_support,
            cached_info: OnceLock::new(),
        }
    }
}

#[async_trait]
impl CliAgent for GenericCliAgent {
    fn info(&self) -> CliAgentInfo {
        self.cached_info
            .get_or_init(|| {
                let id: &'static str = Box::leak(self.id.clone().into_boxed_str());
                let display_name: &'static str =
                    Box::leak(self.display_name.clone().into_boxed_str());
                let binary_name: &'static str =
                    Box::leak(self.binary_name.clone().into_boxed_str());
                let binary_names: &'static [&'static str] =
                    Box::leak(vec![binary_name].into_boxed_slice());
                let version_args: &'static [&'static str] = Box::leak(
                    self.version_args
                        .iter()
                        .map(|s| Box::leak(s.clone().into_boxed_str()) as &'static str)
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                );
                CliAgentInfo {
                    id,
                    display_name,
                    binary_names,
                    version_args,
                    homepage: "",
                    has_verified_noninteractive_mode: self.has_noninteractive_support,
                }
            })
            .clone()
    }

    async fn detect(&self, runner: &dyn CommandRunner) -> CliDetectionResult {
        let Some(binary_path) = runner.which(&self.binary_name).await else {
            return CliDetectionResult::not_installed();
        };

        let version = if self.version_args.is_empty() {
            None
        } else {
            match runner.run(&self.binary_name, &self.version_args, None).await {
                Ok(output) => parse_version(&output.stdout_utf8_lossy()),
                Err(_) => None,
            }
        };

        // No generic login-status convention exists across arbitrary
        // user-configured CLIs.
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
        want_json: bool,
    ) -> Result<CliInvocation, UnsupportedMode> {
        if !self.has_noninteractive_support {
            return Err(UnsupportedMode(format!(
                "{} is not configured with a non-interactive invocation convention.",
                self.display_name
            )));
        }

        let mut args = Vec::new();
        if let Some(flag) = &self.prompt_flag {
            args.extend(flag.split_whitespace().map(|s| s.to_string()));
        }
        args.push(prompt.to_string());

        if want_json
            && let Some(flag) = &self.json_flag {
                args.extend(flag.split_whitespace().map(|s| s.to_string()));
            }
            // No json_flag configured: silently no-op, mirroring the
            // "unconfirmed, not confirmed-absent" treatment used for
            // Qwen/Antigravity elsewhere in this crate.

        args.extend(self.extra_args.iter().cloned());

        Ok(CliInvocation {
            program: self.binary_name.clone(),
            args,
            cwd: Some(cwd.to_path_buf()),
        })
    }

    fn output_format(&self, want_json: bool) -> OutputFormat {
        if want_json && self.json_flag.is_some() {
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

    fn make_agent() -> GenericCliAgent {
        GenericCliAgent::new(
            "my-custom-cli",
            "My Custom CLI",
            "mycli",
            vec!["--version".to_string()],
            Some("-p".to_string()),
            Some("--output-format json".to_string()),
            vec!["--yolo".to_string()],
            true,
        )
    }

    #[tokio::test]
    async fn detect_not_installed() {
        let runner = FakeCommandRunner::new();
        let result = make_agent().detect(&runner).await;
        assert!(!result.installed);
    }

    #[tokio::test]
    async fn detect_installed_parses_version() {
        let runner = FakeCommandRunner::new()
            .with_which("mycli", "/usr/local/bin/mycli")
            .with_run_result("mycli", &["--version"], "mycli version 2.0.1\n", "", 0);
        let result = make_agent().detect(&runner).await;
        assert!(result.installed);
        assert_eq!(result.version.as_deref(), Some("2.0.1"));
        assert_eq!(result.login_status, LoginStatus::Unknown);
    }

    #[test]
    fn info_is_consistent_and_static() {
        let agent = make_agent();
        let info1 = agent.info();
        let info2 = agent.info();
        assert_eq!(info1.id, "my-custom-cli");
        assert_eq!(info1.display_name, "My Custom CLI");
        assert_eq!(info1.binary_names.to_vec(), vec!["mycli"]);
        assert_eq!(info1.version_args.to_vec(), vec!["--version"]);
        assert!(info1.has_verified_noninteractive_mode);
        assert_eq!(info1.id, info2.id);
    }

    #[test]
    fn build_command_plain_text() {
        let agent = make_agent();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("do it", &cwd, false)
            .unwrap();
        assert_eq!(invocation.program, "mycli");
        assert_eq!(
            invocation.args,
            vec!["-p".to_string(), "do it".to_string(), "--yolo".to_string()]
        );
    }

    #[test]
    fn build_command_json() {
        let agent = make_agent();
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("do it", &cwd, true)
            .unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "-p".to_string(),
                "do it".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
                "--yolo".to_string(),
            ]
        );
        assert_eq!(agent.output_format(true), OutputFormat::Json);
    }

    #[test]
    fn build_command_without_prompt_flag_is_positional() {
        let agent = GenericCliAgent::new(
            "id",
            "Name",
            "bin",
            vec![],
            None,
            None,
            vec![],
            true,
        );
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hello", &cwd, false)
            .unwrap();
        assert_eq!(invocation.args, vec!["hello".to_string()]);
    }

    #[test]
    fn build_command_without_json_flag_is_noop_not_error() {
        let agent = GenericCliAgent::new(
            "id",
            "Name",
            "bin",
            vec![],
            Some("-p".to_string()),
            None,
            vec![],
            true,
        );
        let cwd = PathBuf::from("/tmp/project");
        let invocation = agent
            .build_noninteractive_command("hello", &cwd, true)
            .unwrap();
        assert_eq!(invocation.args, vec!["-p".to_string(), "hello".to_string()]);
        assert_eq!(agent.output_format(true), OutputFormat::PlainText);
    }

    #[test]
    fn build_command_unsupported_when_flagged_false() {
        let agent = GenericCliAgent::new("id", "Name", "bin", vec![], None, None, vec![], false);
        let cwd = PathBuf::from("/tmp/project");
        let err = agent
            .build_noninteractive_command("hello", &cwd, false)
            .unwrap_err();
        assert!(!err.0.is_empty());
    }
}
