// Nexus entry point.
//
// Fully wired:
// - `tracing_subscriber` initialization (env-filter based; actual secret
//   redaction happens at each call site via `nexus_core::redact`, not here).
// - A single process-wide multi-threaded `tokio::runtime::Runtime`, whose
//   `Handle` is cloned into `NexusApplication` so `glib::spawn_future_local`
//   closures on the GTK main thread can drive tokio work (either via
//   `handle.spawn(..)` + awaiting the `JoinHandle`, or via `async_channel`).
// - GResource registration, with a graceful (non-panicking) fallback for
//   local/unpackaged development runs where the installed resources path
//   does not exist yet.
// - Construction and `.run()` of `NexusApplication`.
//
// Stubbed / left for later: nothing in this file calls into providers,
// the store, or any view directly -- that all happens inside
// `application::NexusApplication`.

mod application;
mod config;
mod preferences;
mod services;
mod views;
mod window;

use std::path::{Path, PathBuf};

use adw::prelude::*;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        // Default: info for Nexus's own crates, warn for everything else
        // (chatty GTK/Tokio internals), overridable via RUST_LOG.
        EnvFilter::new("warn,nexus=info")
    });

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .init();
}

/// Resolves the GResource bundle to load.
///
/// Order of preference:
/// 1. The installed path baked in at build time (`config::RESOURCES_FILE`).
/// 2. A `nexus.gresource` file sitting next to the running executable, for
///    local `cargo build`/`cargo run` development where nothing has been
///    installed to the Meson prefix.
///
/// Returns `None` (never panics) if neither is found; the caller logs a
/// warning and continues in a degraded dev-mode state where the window's
/// composite templates will fail to load.
fn find_resources_file() -> Option<PathBuf> {
    let installed = Path::new(config::RESOURCES_FILE);
    if installed.exists() {
        return Some(installed.to_path_buf());
    }

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent() {
            let next_to_exe = dir.join("nexus.gresource");
            if next_to_exe.exists() {
                return Some(next_to_exe);
            }
            // Also check the Meson build layout (`_build/src/nexus-resources`
            // style outputs land in the same directory as the build script
            // output in some configurations); best-effort only.
            let build_layout = dir.join("nexus-resources.gresource");
            if build_layout.exists() {
                return Some(build_layout);
            }
        }

    None
}

fn register_resources() {
    match find_resources_file() {
        // `gio::resources_register_from_file` does not exist in gio 0.22 --
        // the real two-step API is `gio::Resource::load(path)` (parses the
        // compiled .gresource file) followed by `gio::resources_register`
        // (registers it with the global GResource namespace the running
        // app's `resource_base_path`/composite templates look things up
        // from).
        Some(path) => match gio::Resource::load(&path) {
            Ok(resource) => {
                gio::resources_register(&resource);
                tracing::info!("Loaded GResource bundle from {}", path.display());
            }
            Err(err) => tracing::warn!(
                "Found a GResource bundle at {} but failed to load it: {err}. \
                 The UI will not be able to load its templates.",
                path.display()
            ),
        },
        None => tracing::warn!(
            "No GResource bundle found (checked {} and the executable's directory). \
             Running in a degraded dev-mode state: windows will fail to load their \
             composite templates. Run via Meson/`ninja devel` or build \
             resources/nexus.gresource.xml manually with glib-compile-resources.",
            config::RESOURCES_FILE
        ),
    }
}

fn main() -> glib::ExitCode {
    init_tracing();
    tracing::info!("Starting Nexus {}", config::VERSION);

    // Single process-wide Tokio runtime. Its `Handle` is cloned into
    // `NexusApplication` (see `application.rs`) so UI code can hand off
    // async work without ever creating a second runtime.
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the Tokio runtime");
    let tokio_handle = tokio_runtime.handle().clone();
    // Keep the runtime alive for the whole process; it is intentionally
    // never dropped before `app.run()` returns.
    let _runtime_guard = tokio_runtime;

    register_resources();

    let app = application::NexusApplication::new(tokio_handle);
    app.run()
}
