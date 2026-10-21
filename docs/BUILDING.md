# Building Nexus

## Prerequisites (local build, GNOME Builder or manual)

- Rust toolchain >= 1.92 (edition 2024) — `rustup install stable`.
- Meson >= 0.62, Ninja.
- `blueprint-compiler` (packaged by most distros; also available as a
  Flatpak SDK extension).
- GTK4 (>= 4.14) and libadwaita (>= 1.5) development packages
  (`gtk4-devel`/`libgtk-4-dev`, `libadwaita-devel`/`libadwaita-1-dev`).
- `pkg-config`, `glib2.0-dev` (for `glib-compile-schemas`,
  `glib-compile-resources`).
- `gettext` (for `msgfmt`/`xgettext`, used by Meson's `i18n` module).
- SQLite is vendored by `rusqlite`'s `bundled` feature — no system
  `libsqlite3-dev` is required.

## Building with GNOME Builder

1. Open Builder, "Clone Repository", point it at this checkout (or open
   the folder directly if already cloned).
2. Builder detects `meson.build` and configures automatically using its
   bundled Flatpak SDK (`org.gnome.Sdk` + the `org.freedesktop.Sdk.Extension.rust-stable`
   extension, which Builder installs on first use if missing).
3. Press the Run button (▶) or `Ctrl+F5`. Builder builds inside the same
   sandbox the Flatpak manifest describes, so this is the closest local
   approximation to the real Flathub build.

## Building manually with `flatpak-builder` (closest to the real Flathub build)

```sh
flatpak install -y flathub org.gnome.Sdk//47 org.gnome.Platform//47 \
  org.freedesktop.Sdk.Extension.rust-stable//24.08
# ^ Check the actual current numbers first:
#   flatpak remote-info flathub org.gnome.Platform
#   The manifest's "runtime-version" must match what you install.

# 1. Generate Cargo.lock (needed before vendoring) and the vendored
#    cargo-sources.json Flatpak needs to build fully offline:
cargo generate-lockfile
git clone https://github.com/flatpak/flatpak-builder-tools.git /tmp/flatpak-builder-tools
python3 /tmp/flatpak-builder-tools/cargo/flatpak-cargo-generator.py \
  Cargo.lock -o build-aux/flatpak/cargo-sources.json

# 2. Build and install into a local repo, then run:
flatpak-builder --user --install --force-clean _flatpak-build \
  build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json
flatpak run io.github.GITHUB_OWNER.Nexus
```

Notes:
- `flatpak-cargo-generator.py` lives in the community
  `flatpak-builder-tools` repository (not shipped with `flatpak-builder`
  itself); there is no vendored copy of it in this repository.
- Re-run step 1 every time `Cargo.lock` changes (new/updated dependency).
  A stale `cargo-sources.json` will either fail the offline build or
  silently use outdated dependency versions.
- The manifest's `nexus` module currently sources from `{"type": "dir",
  "path": "../.."}` (the local checkout) for day-to-day local builds. A
  real Flathub submission must change this to a `"type": "git"` source
  pinned to a tagged release commit — see `docs/FLATHUB_SUBMISSION.md`.

## Building outside Flatpak/Meson (development-only, for `cargo check`)

Every crate under `crates/` is a plain library crate with no GTK
dependency and can be checked/tested directly:

```sh
cargo check --workspace --exclude nexus   # everything except the GTK binary
cargo test --workspace --exclude nexus
```

The `nexus` binary package itself additionally needs `src/config.rs`,
which only Meson's `configure_file` step produces (see
`src/config.rs.in` and `src/meson.build`) — running `cargo build` on the
`nexus` package directly, without ever having run Meson once, will fail
with a missing-module error for `config.rs`. Run `meson setup _build &&
meson compile -C _build` at least once first; afterwards `cargo build`/
`cargo run` on the `nexus` package works directly since `src/config.rs`
is left in place in the source tree (gitignored, regenerated on the next
Meson build).

## Running the test suite

```sh
cargo test --workspace --exclude nexus
```

Tests that need a real Secret Service / portal session (in
`nexus-secrets`) are marked `#[ignore]` and do not run by default — run
them explicitly with `cargo test -p nexus-secrets -- --ignored` on a
desktop session with `gnome-keyring` (or another Secret Service provider)
running.

## Verification before sending a pull request

```sh
cargo fmt --all -- --check
cargo clippy --workspace --exclude nexus --all-targets -- -D warnings
cargo test --workspace --exclude nexus
appstreamcli validate --pedantic data/io.github.GITHUB_OWNER.Nexus.metainfo.xml.in
desktop-file-validate data/io.github.GITHUB_OWNER.Nexus.desktop.in
flatpak-builder-lint manifest build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json
flatpak-builder-lint appstream data/io.github.GITHUB_OWNER.Nexus.metainfo.xml.in
```

`flatpak-builder-lint` is a separate pip-installable tool
(`pip install flatpak-builder-lint`) maintained by the Flathub team; it is
not bundled with `flatpak-builder`.
