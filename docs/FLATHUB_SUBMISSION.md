# Submitting Nexus to Flathub

This is a checklist, not a guarantee of acceptance — Flathub review is a
real human review and may ask for changes beyond what is listed here.

## 0. Rename the placeholder

Every file in this repository uses the literal placeholder string
`GITHUB_OWNER` (in the app ID `io.github.GITHUB_OWNER.Nexus`, file names,
the `resource-base-path`, the manifest, `Cargo.toml`'s `repository`
field, `metainfo.xml.in`'s URLs, etc). Before anything else, do a
project-wide search and replace `GITHUB_OWNER` with your real GitHub
username/organization, and rename every file whose name contains it
(`data/io.github.GITHUB_OWNER.Nexus.*`,
`build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json`,
`data/icons/hicolor/{scalable,symbolic}/apps/io.github.GITHUB_OWNER.Nexus*`).
Double-check `resources/nexus.gresource.xml`'s `prefix` attribute and
every `gio::File::for_path`/`gtk::Builder::from_resource` string in
`src/*.rs` that hardcodes `/io/github/GITHUB_OWNER/Nexus/...` — a
resource-path mismatch fails at runtime with a confusing "resource not
found" error, not a compile error.

Also reconsider the app name itself: "Nexus" is a common, generic word —
search Flathub's existing catalog and trademark databases before
committing to it for a real submission; the spec called it "provisional
and easy to change" for exactly this reason.

## 1. Prepare the release

1. Pick a real screenshot: run the app, take a real screenshot of the
   chat view (or whichever view best represents the app), host it
   somewhere stable (commonly: attach it to a tagged GitHub release, or
   commit it under e.g. `data/screenshots/` and reference the raw GitHub
   URL), and replace the placeholder `<screenshot>` entry in
   `data/io.github.GITHUB_OWNER.Nexus.metainfo.xml.in`.
2. Tag a release in your GitHub repo (e.g. `v0.1.0`) once the code is in
   a state you're confident building reproducibly.
3. Update `build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json`'s `nexus`
   module `sources` entry from the local `{"type": "dir", "path": "../.."}`
   to:
   ```json
   {
     "type": "git",
     "url": "https://github.com/GITHUB_OWNER/nexus.git",
     "tag": "v0.1.0",
     "commit": "<the exact commit SHA the tag points to>"
   }
   ```
   (Flathub's own linter checks for a pinned `tag`+`commit` pair, not a
   branch name, so the build is reproducible.)
4. Regenerate `build-aux/flatpak/cargo-sources.json` from the `Cargo.lock`
   committed at that exact tag (see `docs/BUILDING.md`), and commit it (it
   is gitignored for day-to-day local development to avoid it going stale
   silently — a release commit should include a freshly regenerated one).
5. Re-check `build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json`'s
   `runtime-version` against the actual current Flathub GNOME runtime:
   `flatpak remote-info flathub org.gnome.Platform`.

## 2. Local verification before opening a PR

Run everything in `docs/BUILDING.md`'s "Verification before sending a
pull request" section, plus:

```sh
flatpak-builder --user --install --force-clean _flatpak-build \
  build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json
flatpak run io.github.GITHUB_OWNER.Nexus
```

Confirm manually:
- The app launches and its window appears under both Wayland and (if you
  can test it) Xorg/XWayland.
- Dark and light `libadwaita` themes both render correctly.
- Preferences lets you (attempt to) connect at least one provider and
  shows a sensible state when the Secret portal is unavailable.
- With the `--talk-name=org.freedesktop.Flatpak` permission present but
  the in-app "run CLI agents installed on this computer" toggle left OFF
  (the default), confirm the app never actually calls `flatpak-spawn`
  — e.g. with `flatpak run --command=sh io.github.GITHUB_OWNER.Nexus` you
  can inspect that no such subprocess is spawned during normal use.

## 3. Fork and submit to `flathub/flathub`

1. Fork `https://github.com/flathub/flathub` on GitHub.
2. Create a new branch named exactly after the app ID:
   `io.github.GITHUB_OWNER.Nexus`.
3. Add exactly one file to the root of that branch: the manifest
   (`io.github.GITHUB_OWNER.Nexus.json`, or `.yml`/`.yaml` — Flathub's
   initial-submission process only wants the manifest itself at this
   stage, not the full repo; the actual build sources come from the `git`
   source entry inside it pointing at your own repository).
4. Open a pull request against `flathub/flathub` from that branch.
   Flathub's bot (`flathubbot`) will comment with automated build/lint
   results within a few minutes.
5. A human reviewer from the Flathub team will review the PR. Be prepared
   to answer, in the PR thread:
   - Why `--talk-name=org.freedesktop.Flatpak` is requested, and that it
     is gated behind an off-by-default in-app setting (point them at
     `build-aux/flatpak/PERMISSIONS.md`, which was written specifically
     to answer this).
   - Why `--share=network` is required even for "local-only" use (local
     model servers are still reached over loopback TCP).
   - Confirmation that no telemetry is sent (point them at the Privacy
     preferences page and the `telemetry-enabled` GSettings key, which
     defaults to `false` and is never read by any network code path).
6. Once the reviewer approves and merges, Flathub's build infrastructure
   builds and publishes the app; it typically appears on flathub.org
   within a day.

## 4. After acceptance

- Keep `appstream`'s `<releases>` section in
  `data/io.github.GITHUB_OWNER.Nexus.metainfo.xml.in` up to date with
  every tagged release — Flathub surfaces this as the app's changelog.
- Flathub re-builds on every push to the branch in your `flathub/flathub`
  fork that holds the manifest; updating the app means updating the `tag`
  /`commit` (and regenerating `cargo-sources.json`) in that manifest and
  pushing to that branch, not just tagging a new release in your own repo.
