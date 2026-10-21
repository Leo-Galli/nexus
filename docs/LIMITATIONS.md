# Known limitations, unverified facts, and required pre-submission checks

This document is the single place collecting every "unverified" or
"best-effort" note left across the codebase, plus the manual checks a
maintainer must run before trusting a build or submitting to Flathub.
Where the code itself carries a `// VERIFIED:` / `// UNVERIFIED:` comment,
that comment is the source of truth; this file indexes them.

## Build system / dependencies

- **Rust toolchain / edition**: the workspace targets edition 2024 /
  Rust >= 1.92 because `gio`/`glib` 0.22.x (the versions compatible with
  the current `libadwaita` 0.9.x / `gtk4` 0.11.x / `vte4` 0.10.x set,
  verified against crates.io on 2026-10-21) declare that MSRV.
  **Verified**: `cargo check --bin nexus` and `cargo test --workspace`
  both succeed end-to-end (rustc/cargo 1.98.1) — the full workspace
  (every crate plus the GTK binary) compiles and **all tests pass**
  (158 passed, 3 intentionally `#[ignore]`d pending a real Secret
  Service/portal session; zero failures). This found and fixed several
  real bugs during integration: a nonexistent `anyhow_lite` dependency,
  a missing `PartialEq` derive, a missing parent-row foreign-key setup in
  automation engine tests, stray `futures_core`/`Trigger` imports in
  tests, and three provider-constructor call-site mismatches in
  `src/services.rs` (the real `OpenAiProvider`/`AnthropicProvider`/
  `GeminiProvider`/`OllamaProvider` constructors take different
  arguments than first assumed). `cargo clippy --workspace --all-targets`
  was also run and its safe auto-fixes applied; only style-level lints
  remained (no correctness warnings).
- **Not verified in this environment**: `meson`, `ninja`,
  `blueprint-compiler`, and `flatpak-builder` are not installed here, so
  the Meson build (`src/meson.build`'s `blueprint-compiler batch-compile`
  invocation and the `cargo-build` custom target calling
  `build-aux/cargo.sh`), the actual `.blp` → `.ui` compilation, and the
  Flatpak build have **not** been exercised end-to-end. These follow
  patterns seen in real GNOME Rust app templates (Fractal/Shortwave-
  style) but their exact CLI flags should be re-verified against current
  `--help` output the first time a real build is attempted; see
  `docs/BUILDING.md`.
- **GNOME runtime version**: `build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json`
  pins `org.gnome.Platform`/`org.gnome.Sdk` version `47`. Re-check this
  against `flatpak remote-info flathub org.gnome.Platform` before
  building — GNOME ships a new numbered runtime yearly and 47 may already
  be superseded by the time you read this.
- **`cargo-sources.json`** referenced by the Flatpak manifest does not
  exist in this repository yet. A real `Cargo.lock` now does exist (`cargo
  check`/`cargo test` generated and committed one); regenerate
  `cargo-sources.json` from it with `flatpak-cargo-generator.py` before a
  real Flatpak build (see `docs/BUILDING.md`) — do not reuse a stale copy
  if `Cargo.lock` changes afterward.
- **`src/services.rs`** (the `Services` struct bundling the store, secret
  store, provider/CLI-agent registries, and pricing table) is wired into
  the build (`mod services;` in `main.rs`) and compiles, but is **not yet
  constructed or used anywhere** — `NexusApplication` still opens the
  store and a secret store through its own separate `OnceLock` fields.
  Replacing those with `Services::bootstrap` and threading the result
  through to `preferences.rs` and every view is the next integration
  step; see the doc comment at the top of `src/services.rs`.

## Providers (`crates/nexus-providers`)

- **Perplexity**: `list_models` is intentionally unimplemented
  (`ProviderError::Unsupported`) because a standard `/models` listing
  endpoint for Perplexity's API was not confirmed; chat still works
  against a user-selected model id.
- **Qwen/DashScope OpenAI-compatible mode**: the international endpoint
  (`dashscope-intl.aliyuncs.com`) is used by default; mainland-China
  accounts may need `dashscope.aliyuncs.com` instead — exposed as an
  override via the custom-endpoint path, not auto-detected.
- **Cohere v2 streaming event shape**: implemented defensively
  (unrecognized `"type"` values are ignored rather than erroring) because
  the exact v2 newline-delimited streaming event names were not fully
  verified against current docs at implementation time.
- **Azure OpenAI**: `list_models` returns only the single configured
  deployment (Azure has no universal "list my deployments" endpoint
  usable the same way across all accounts/API versions) — this is by
  design, not a bug. The `api-version` query parameter has a
  constructor-supplied default that Azure revs periodically; expose it as
  a user-editable setting, do not hardcode trust in it staying current.
- **Amazon Bedrock**: an explicit, honest stub. Bedrock requires AWS
  SigV4 request signing and typically full AWS credentials (not a single
  API key), which needs a dedicated crate (e.g. a maintained
  `aws-sigv4`/`aws-sdk-bedrockruntime`) not present in this workspace.
  `status()`/`list_models()`/`send_chat()` all return a clear
  "not implemented" error rather than silently failing against real AWS.
- **llama.cpp server**: its OpenAI-compatibility surface can vary by build
  and startup flags; parsing failures are surfaced as a soft
  `ProviderError::InvalidResponse` with guidance text rather than a panic.
- Local-server discovery (`providers::discovery`) **only** probes
  `localhost`/`127.0.0.1` on the three documented default ports (Ollama
  11434, LM Studio 1234, llama.cpp server 8080) — it deliberately never
  scans any LAN subnet or non-loopback address.

## CLI agents (`crates/nexus-cli-agents`)

- **Version detection** is only implemented where a version flag was
  confirmed in official docs (Qwen Code `--version`, Cursor's `agent
  --version`); Claude Code, Codex CLI, Gemini CLI, Antigravity, Aider, and
  GitHub Copilot CLI always report `version: None` rather than guessing a
  flag.
- **Login-status detection** is only implemented for OpenAI Codex CLI
  (`codex login status`) and Cursor (`agent status`); every other agent
  always reports `LoginStatus::Unknown`.
- **Windsurf** has **no implementation** at all: research against
  official sources found no standalone Windsurf agent CLI (only
  unofficial third-party wrappers). Users can still add it by hand via
  the generic, user-configurable `GenericCliAgent`.
- **Gemini CLI**'s documented `--output-format json` mode is reported in
  a public GitHub issue (google-gemini/gemini-cli#9009) as sometimes
  unreliable in practice; implemented as documented regardless, with the
  caveat surfaced via `GeminiCliAgent::RELIABILITY_NOTE`.
- **Qwen Code** and **Google Antigravity (`agy`)**: no JSON output flag
  could be confirmed in official docs. `build_noninteractive_command`
  silently never adds one (never errors on `want_json: true`) since
  absence is *unconfirmed*, not *confirmed*.
- **Aider**: confirmed to have **no** JSON output mode at all (not merely
  unverified) — `build_noninteractive_command` explicitly returns an
  "unsupported" error when JSON is requested.
- **Cursor CLI**: the real binary is `agent` (the installer also creates a
  `cursor-agent` symlink); `CliAgent::build_noninteractive_command` always
  targets `"agent"` as the program name regardless of which name
  `detect()` actually resolved, since the trait has no channel to pass
  that back — a known, documented simplification.
- Every agent that supports an "apply changes without confirmation" flag
  (Cursor `--force`, Aider `--yes-always`, Copilot CLI `--allow-all-tools`)
  defaults that behavior **off**; Nexus's own confirmation UI is expected
  to gate file-touching actions regardless.

## Secrets (`crates/nexus-secrets`)

- Built against `oo7` 0.3.3; the exact call sites (`Keyring::new`,
  `create_item`, `search_items`, `Item::secret/attributes/delete`) compiled
  successfully against the real crate, but their *behavioral* edge cases
  (portal-vs-Secret-Service fallback, cross-version `delete` semantics)
  were not exercised against a live backend in this environment — the
  `#[ignore]`d tests in that crate must be run on a real desktop session
  (`cargo test -p nexus-secrets -- --ignored`) before trusting them fully.

## Flatpak packaging

See `build-aux/flatpak/PERMISSIONS.md` for the full, per-permission
breakdown. Highlights:
- Whether Flatpak's network sandbox denial of `--share=network` also
  blocks loopback traffic to local model servers (Ollama/LM
  Studio/llama.cpp) is stated as *likely but not independently verified*
  — local-only use probably still requires `--share=network` to be
  granted; this needs confirming before telling a reviewer or user
  otherwise.
- No OARS 1.1 attribute precisely captures "this app sends your prompts
  to whatever third-party AI provider you configure" — every applicable
  category is marked `none` with an XML comment explaining why, rather
  than inventing a rating value.
- `flathub.json` only declares `only-arches`; other commonly-seen keys
  were deliberately omitted as unverified rather than guessed.
- `--talk-name=org.freedesktop.Flatpak` (host CLI execution) must be
  statically present in the manifest for the feature to be possible at
  all (Flatpak has no in-app-togglable D-Bus permission); Nexus's own
  `enabled-host-cli-execution` GSetting (default `false`) gates *use* of
  it in application code. Whether Flathub reviewers accept this
  justification is a real review-outcome judgment call, not guaranteed.

## General

- Every price in `data/pricing.toml` must come from the provider's own
  current official pricing page — the file ships empty on purpose (see
  its header comment) and cost estimates show "unknown" rather than a
  guessed number for anything not listed.
- The `GITHUB_OWNER` placeholder appears in dozens of files; see
  `docs/FLATHUB_SUBMISSION.md` step 0 before any real submission.
