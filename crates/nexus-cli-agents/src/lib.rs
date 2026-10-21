//! `CliAgent` implementations for coding-agent CLIs, plus the
//! `CommandRunner` implementations that decide *where* a process actually
//! executes (directly on the host, or via `flatpak-spawn --host` when
//! Nexus is sandboxed and the user has opted in).

pub mod agents;
pub mod detect;
pub mod runner;

#[cfg(test)]
pub mod test_support;

pub use runner::{FlatpakHostCommandRunner, HostCommandRunner};
