use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nexus_core::{CommandOutput, CommandRunner};
use tokio::process::Command;

/// Runs commands directly on the same system Nexus is running on. Used
/// when Nexus is not sandboxed (e.g. during local development) or on
/// distros where it is installed outside Flatpak.
pub struct HostCommandRunner;

#[async_trait]
impl CommandRunner for HostCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> std::io::Result<CommandOutput> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        let output = cmd.output().await?;
        Ok(CommandOutput {
            status_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    async fn which(&self, program: &str) -> Option<PathBuf> {
        which::which(program).ok()
    }
}

/// Runs commands on the **host** system from inside the Flatpak sandbox,
/// via `flatpak-spawn --host`. Requires the `--talk-name=org.freedesktop.
/// Flatpak` permission declared in the manifest, and must only be
/// constructed after the user has explicitly enabled "Run CLI agents
/// installed on this computer" in Preferences — see
/// `docs/FLATPAK_PERMISSIONS.md` for the justification given to Flathub
/// reviewers and the degraded behaviour when the permission is refused by
/// the portal/sandbox at runtime.
pub struct FlatpakHostCommandRunner;

#[async_trait]
impl CommandRunner for FlatpakHostCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        cwd: Option<&Path>,
    ) -> std::io::Result<CommandOutput> {
        let mut cmd = Command::new("flatpak-spawn");
        cmd.arg("--host");
        if let Some(cwd) = cwd {
            cmd.arg(format!("--directory={}", cwd.display()));
        }
        cmd.arg(program);
        cmd.args(args);
        let output = cmd.output().await?;
        Ok(CommandOutput {
            status_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    async fn which(&self, program: &str) -> Option<PathBuf> {
        // `flatpak-spawn --host which <program>` resolves against the
        // host's PATH, not the sandbox's.
        let output = Command::new("flatpak-spawn")
            .arg("--host")
            .arg("which")
            .arg(program)
            .output()
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            None
        } else {
            Some(PathBuf::from(path))
        }
    }
}
