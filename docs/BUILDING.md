# Building and running Nexus

## Prerequisites (local, non-Flatpak build)

- Rust stable toolchain (edition 2024, see `rust-version` in `Cargo.toml`
  for the minimum supported version).
- Meson >= 0.62, Ninja.
- `blueprint-compiler` (compiles `.blp` UI sources to `.ui`).
- GTK4 (>= 4.14) and libadwaita (>= 1.5) development packages, and their
  `pkg-config` files.
- `vte4` (or `vte-2.91-gtk4`, package name varies by distro) development
  package, for the embedded terminal used by the CLI agents view.
- `gettext` (for `.po`/`.mo` handling via Meson's `i18n` module).
- `sqlite3` development headers are **not** required: `rusqlite` is built
  with the `bundled` feature, which compiles its own SQLite.

On Fedora/GNOME-oriented distros, something like:

```sh
sudo dnf install meson ninja-build blueprint-compiler gtk4-devel \
  libadwaita-devel vte291-gtk4-devel gettext rustup
```

On Arch Linux:

```sh
sudo pacman -S meson ninja blueprint-compiler gtk4 libadwaita vte4 gettext rust
```

Package names and exact VTE package naming vary by distro — check your
distro's package search if the above doesn't match.

## Building with GNOME Builder

1. Open GNOME Builder, "Clone Repository", point it at this repository
   (or "Open Project" on a local checkout).
2. Builder detects the `meson.build` at the repository root and offers to
   build inside a Flatpak SDK matching `build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json`
   automatically — accept that; it handles the GNOME runtime/SDK and
   `blueprint-compiler` for you without touching your host system.
3. Press the Run button (or Ctrl+F5/the "play" icon in the header bar).
   The first build will take a while since Cargo has to fetch and build
   every dependency; subsequent builds are incremental.
4. Builder's "Build > Export Bundle" action (or the equivalent menu entry
   in your Builder version) can produce a `.flatpak` bundle for sharing.

## Building manually with `meson` + `ninja` (host build, no Flatpak)

```sh
meson setup _build
ninja -C _build
sudo ninja -C _build install   # installs into your configured prefix
nexus                          # or: _build/src/nexus, if you skip install
```

To rebuild after editing Rust code only (no `.blp`/`.xml.in` changes),
`ninja -C _build` re-invokes `cargo build` via the `cargo-build` custom
target and is fast after the first build.

Use `meson setup _build -Dprofile=development` for a development build:
this appends a `-devel` suffix to the version string and is intended to
be paired with a differently-colored app icon convention (not yet wired
up in `data/meson.build` — a `Devel`-suffixed desktop file/icon variant
is a reasonable follow-up once the base app is stable, per the usual
GNOME HIG convention for development builds).

## Building with `flatpak-builder` (the real Flathub build path)

1. Install `flatpak-builder` and add the Flathub and
   `org.gnome.Platform`/`org.gnome.Sdk` runtime/SDK at the version
   referenced in the manifest:

   ```sh
   flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
   flatpak install flathub org.gnome.Platform//47 org.gnome.Sdk//47
   ```

   (Check `flatpak remote-info flathub org.gnome.Platform` first and use
   whatever the current stable runtime version actually is — "47" in this
   repository is a placeholder picked at the time the manifest was
   written and may already be out of date; see `docs/LIMITATIONS.md`.)

2. Generate a `Cargo.lock` (required once, and whenever dependencies
   change) and the vendored Flatpak Cargo sources:

   ```sh
   cargo generate-lockfile        # or: cargo build, which also writes Cargo.lock
   python3 flatpak-cargo-generator.py Cargo.lock \
     -o build-aux/flatpak/cargo-sources.json
   ```

   `flatpak-cargo-generator.py` comes from
   `https://github.com/flatpak/flatpak-builder-tools` (under
   `cargo/flatpak-cargo-generator.py`) — clone that repository separately,
   it is not vendored into this one.

3. Build and install into a local test repo:

   ```sh
   flatpak-builder --user --install --force-clean _flatpak-build \
     build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json
   ```

4. Run it:

   ```sh
   flatpak run io.github.GITHUB_OWNER.Nexus
   ```

5. Lint before submitting anywhere:

   ```sh
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest \
     build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json
   ```

   (`flatpak-builder-lint` ships inside the `org.flatpak.Builder` Flatpak;
   install it with `flatpak install flathub org.flatpak.Builder` if you
   don't already have it.)

## Running the test suite

```sh
cargo test --workspace
```

Each crate's tests were verified independently during development (see
`docs/LIMITATIONS.md` item 6.10 for the one remaining gap: a full
workspace-wide `cargo test` has not yet been run in this environment,
since neither `meson` nor `flatpak-builder` are installed here — run it
yourself before relying on this scaffold).

`nexus-secrets`'s Secret Service/portal integration tests are marked
`#[ignore]` since they need a real keyring/portal session; run them
explicitly with `cargo test -p nexus-secrets -- --ignored` on a machine
with `gnome-keyring-daemon`/a portal implementation available.

## Submitting to Flathub

1. Fork `https://github.com/flathub/flathub` on GitHub.
2. Create a new branch, e.g. `new-app-nexus`, and open a PR against the
   `flathub` repo's `new-pr` branch (Flathub's current submission
   workflow asks new-app PRs to target a specific branch — check
   Flathub's own up-to-date submission docs at
   `https://docs.flathub.org/docs/for-app-authors/submission` for the
   exact branch name and process at the time you submit, since Flathub's
   process has changed over time and should not be assumed frozen).
3. Add this project's manifest (renamed/placed per Flathub's expected
   layout: typically the manifest file is named exactly
   `<app-id>.json` at the repository root of the Flathub submission repo)
   pointing at a real `git` source with a pinned release tag (see
   `docs/LIMITATIONS.md` checklist item 3).
4. Flathub's CI will build the manifest and run `flatpak-builder-lint`
   automatically; fix anything it flags.
5. A human reviewer will comment on the PR, most likely asking about:
   - The `--talk-name=org.freedesktop.Flatpak` permission — answer using
     the justification in `build-aux/flatpak/PERMISSIONS.md`, emphasizing
     that it's off by default behind a GSetting and the UI shows the
     exact command before running it.
   - Why network is required even for "local-only" use (loopback traffic
     to Ollama/LM Studio/llama.cpp still needs `--share=network`).
   - The placeholder screenshot and `GITHUB_OWNER`/app-id placeholders —
     make sure these are already fixed (checklist items 4-5) before you
     submit, not after a reviewer points them out.
6. Respond to review comments promptly and update the PR branch (new
   commits, not force-pushes over review history, unless a reviewer asks
   for a squash) until it is approved and merged. Once merged, Flathub
   builds and publishes automatically; subsequent releases are new PRs
   (or, once set up, automated version-bump PRs) updating the pinned
   `tag`/commit in the manifest.
