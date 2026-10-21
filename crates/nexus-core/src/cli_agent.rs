use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Stable identifier + metadata for a CLI agent, independent of whether it
/// is currently installed.
///
/// `Serialize`-only: every field is `&'static str`/`&'static [&'static
/// str]` (these are compiled-in constants, never user data), which cannot
/// soundly implement `Deserialize` for an arbitrary input lifetime, so
/// this type is never read back from JSON — only written to it (e.g. for
/// a debug dump).
#[derive(Debug, Clone, Serialize)]
pub struct CliAgentInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Binary names to look for on PATH, in priority order.
    pub binary_names: &'static [&'static str],
    /// Arguments that print a version string, e.g. `["--version"]`.
    pub version_args: &'static [&'static str],
    pub homepage: &'static str,
    /// `true` only when a genuine non-interactive / scriptable mode has
    /// been verified against the tool's own documentation. `false` means
    /// Nexus will only offer the interactive-terminal (VTE) integration.
    pub has_verified_noninteractive_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
    Unknown,
    LoggedIn,
    LoggedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliDetectionResult {
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub login_status: LoginStatus,
}

impl CliDetectionResult {
    pub fn not_installed() -> Self {
        Self {
            installed: false,
            binary_path: None,
            version: None,
            login_status: LoginStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.status_code == Some(0)
    }

    pub fn stdout_utf8_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }
}

/// Abstracts *where* a process actually runs: directly on the host when
/// Nexus is not sandboxed (or running un-packaged), or via
/// `flatpak-spawn --host` when inside the Flatpak sandbox and the user has
/// explicitly opted in to host command execution. Tests use a fake runner
/// that records invocations instead of touching the real process table.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> std::io::Result<CommandOutput>;

    /// Locate a binary by name using the same resolution the eventual
    /// `run` call will use (host PATH when spawning via
    /// `flatpak-spawn --host`, sandbox PATH otherwise).
    async fn which(&self, program: &str) -> Option<PathBuf>;
}

/// A ready-to-execute invocation for a non-interactive CLI call.
#[derive(Debug, Clone)]
pub struct CliInvocation {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// Whether a CLI's non-interactive output is line/plain text or a
/// documented JSON format Nexus can render as a structured view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    PlainText,
    Json,
}

#[async_trait]
pub trait CliAgent: Send + Sync {
    fn info(&self) -> CliAgentInfo;

    /// Resolve install state, version string, and (best-effort) login
    /// status using the provided runner.
    async fn detect(&self, runner: &dyn CommandRunner) -> CliDetectionResult;

    /// Build the command line for a single non-interactive turn, if the
    /// CLI documents one. Returns `Err(Unsupported)` when
    /// `has_verified_noninteractive_mode` is `false` — callers must fall
    /// back to the interactive VTE terminal integration in that case.
    fn build_noninteractive_command(
        &self,
        prompt: &str,
        cwd: &Path,
        want_json: bool,
    ) -> Result<CliInvocation, UnsupportedMode>;

    fn output_format(&self, want_json: bool) -> OutputFormat;
}

#[derive(Debug, Clone)]
pub struct UnsupportedMode(pub String);

impl std::fmt::Display for UnsupportedMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for UnsupportedMode {}
