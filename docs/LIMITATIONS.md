# Known limitations, unverified details, and pre-submission checks

This document is the authoritative, up-to-date list the spec asked for. It
is compiled from the actual implementation work, not written in advance —
every item below reflects something a real pass over the code found.
Treat anything not listed here as either fully implemented or genuinely
unknown (ask before assuming).

## 1. Provider implementations

| Provider | Status | Caveat |
|---|---|---|
| OpenAI | Implemented (chat streaming, models list) | `/v1/models` does not reliably return context-window/pricing metadata; those fields are always `None` from this provider, enriched only via `nexus-stats`'s pricing table. |
| Anthropic | Implemented (chat streaming, models list) | No native tool-calling (function calling) wiring yet — `ToolDefinition`s are accepted by the trait but not translated into Anthropic's `tools`/`tool_use` request/response shape. `Role::Tool` messages are folded into a `user` message with a `tool_result` block as a stand-in. |
| Google Gemini | Implemented (chat streaming, models list) | Same tool-calling gap as Anthropic: no `functionCall`/`functionResponse` modeling yet. |
| Mistral, Groq, DeepSeek, OpenRouter, Together, xAI Grok, Qwen/DashScope | Implemented via the generic OpenAI-compatible provider | Each is a named preset base URL. Qwen/DashScope defaults to the international endpoint (`dashscope-intl.aliyuncs.com`); mainland-China accounts must use the custom-URL path with `dashscope.aliyuncs.com` instead. |
| Perplexity | Partially implemented | `send_chat` works against the OpenAI-compatible shape, but `list_models` deliberately returns `ProviderError::Unsupported` — the `/models` listing endpoint's existence/shape was not verified against official docs, so Nexus does not guess it. The Preferences UI should link to Perplexity's own model-list documentation instead. |
| Cohere | Implemented, marked UNVERIFIED | Cohere v2's exact streaming event field layout (`content-delta`/`message-end` nesting) was not verified against a live account; parsing is defensive (unknown event types/missing fields are ignored rather than erroring) but should be re-checked against a real response before relying on it. |
| Azure OpenAI | Implemented with caveats | No universal "list my deployments" endpoint is used (none is guaranteed available across all resource/API-version/tier combinations) — `list_models` returns a single synthetic entry built from the user-configured deployment name. The default `api-version` string will go stale over time; the Preferences UI must let the user override it. |
| Amazon Bedrock | Honest stub, not functional | Requires AWS SigV4 request signing and typically AWS credential profiles, not a bearer token. No SigV4/AWS SDK crate is in the dependency tree. `status()`/`list_models()`/`send_chat()` all report clearly that Bedrock is unimplemented rather than silently failing against real AWS. Implementing this is future work requiring a dependency decision (e.g. `aws-sdk-bedrockruntime` or a standalone SigV4 crate). |
| Ollama | Implemented | Local-only, `AuthMethod::None`, native NDJSON `/api/chat` + `/api/tags`. |
| LM Studio | Implemented | OpenAI-compatible local server. |
| llama.cpp server | Implemented, marked variable | The built-in `server` example's OpenAI-compatibility surface can vary by build and launch flags; parsing failures degrade to a clear `ProviderError::InvalidResponse` rather than a panic. |
| Local discovery | Implemented | Probes only `127.0.0.1`/`localhost` on the three default ports (11434, 1234, 8080) with a short timeout. Deliberately does **not** scan any LAN subnet or non-loopback address — that would be a privacy/security overreach beyond what was asked. |

## 2. CLI agent implementations

Facts below come from a dedicated research pass against official
documentation on 2026-10-21 — see citations kept in each module's doc
comment. Nothing here was inferred by analogy between tools.

| CLI | Binary | Non-interactive | JSON output | Login-status probe |
|---|---|---|---|---|
| Claude Code | `claude` | `-p`/`--print` | `--output-format json\|stream-json` (with `-p`) | Not documented anywhere found — always reports `LoginStatus::Unknown`. Version flag also unconfirmed — version is always `None`. |
| OpenAI Codex CLI | `codex` | `codex exec` | `codex exec --json` (JSONL) | `codex login status` / `codex whoami`, implemented. |
| Gemini CLI | `gemini` | `-p`/`--prompt` | `--output-format json` (alias `-o json`) | Documented, but a GitHub issue (google-gemini/gemini-cli#9009) reports this JSON mode is sometimes unreliable in practice — implemented as documented, with that caveat noted in the module. |
| Qwen Code | `qwen` | `-p` | **Unverified** — no JSON flag found in official docs; `want_json` is silently ignored (plain text is always produced), never a guessed flag. Version flag (`--version`) is confirmed. |
| Cursor CLI | `agent` (the `cursor-agent` name is only an installer-created symlink) | `-p`/`--print` | `--output-format json\|text\|stream-json` | `agent status`, implemented via a heuristic substring check on its output. |
| Windsurf | — | — | — | **No officially documented standalone agent CLI exists** as of this research pass (only unofficial third-party wrappers were found). Not implemented as a built-in `CliAgent`; users can still add it through the generic configurable CLI agent if a future version of Windsurf ships one. |
| Google Antigravity | `agy` | `-p`/`--print`/`--prompt` | **Unverified** — no JSON flag found; same "silently ignore `want_json`" behavior as Qwen Code. |
| Aider | `aider` | `--message`/`-m` | **Confirmed absent** — Aider has no JSON output mode at all. `build_noninteractive_command` returns an explicit error when `want_json: true`, rather than silently ignoring it, since this is a confirmed absence rather than an unverified one. |
| GitHub Copilot CLI | `copilot` (the old `gh copilot` extension is retired) | `-p`/`--prompt` (`--allow-all-tools` for unattended runs, opt-in) | `--output-format=json` (JSONL) | Not implemented — no documented probe found. |

A pre-existing, unrelated bug was found and fixed during this pass:
`nexus_core::cli_agent::CliAgentInfo` originally derived `serde::Deserialize`
on fields typed `&'static str`/`&'static [&'static str]`, which cannot
soundly implement `Deserialize` for an arbitrary input lifetime. The
derive was narrowed to `Serialize`-only (the type is only ever produced in
code, never read back from JSON).

## 3. Flatpak / sandboxing

- **`--talk-name=org.freedesktop.Flatpak`** (host CLI execution): Flatpak
  has no concept of a togglable D-Bus permission — a `finish-args` entry
  either is granted at install time or it isn't. The permission is
  therefore declared statically in the manifest, but Nexus's own code
  never calls `flatpak-spawn --host` unless the user has explicitly
  enabled "Run CLI agents installed on this computer" in Preferences
  (off by default, backed by the `enabled-host-cli-execution` GSetting).
  Expect Flathub reviewers to scrutinize this permission specifically;
  the full justification is in `build-aux/flatpak/PERMISSIONS.md`.
- Whether Flatpak's `--share=network` denial also blocks loopback
  (127.0.0.1) traffic to local model servers (Ollama/LM Studio/llama.cpp)
  was **not independently verified** — if a user denies network entirely,
  local-model functionality may or may not survive; this needs a real
  test with the permission revoked before claiming local-only operation
  works without network.
- The Document portal's folder-access persistence for an **unattended**
  automation folder-watch trigger (no user present to drive a picker
  dialog at the moment of the filesystem event) was **not verified** —
  this needs real-world testing; it may require the user to grant folder
  access once via a picker before the automation is first enabled, with
  the portal persisting that grant for future headless triggers.
- `CARGO_HOME`/offline cargo environment variables in the manifest follow
  the conventional `flatpak-cargo-generator` pattern but were **not
  verified against an actual generator run** (no `Cargo.lock` existed yet
  when the manifest was written). Regenerate `cargo-sources.json` and
  double-check these env vars once a real `Cargo.lock` exists.
- `build-aux/flatpak/flathub.json` only sets `only-arches`, the one key
  verified with confidence; other commonly-referenced keys were
  deliberately omitted rather than guessed.
- No OARS 1.1 attribute precisely captures "sends the text you type to
  whichever third-party AI provider you configure" — all applicable
  categories are set to `none` with an XML comment explaining the gap,
  rather than inventing a rating value.
- The manifest's Flathub source currently points at a local `dir` — it
  **must** become a `type: git` source with a `tag` pinned to a real
  release before actual Flathub submission (see `docs/BUILDING.md`).
- The metainfo's screenshot is a placeholder URL and must be replaced
  with a real, properly hosted screenshot before submission.
- `org.gnome.Platform`/`org.gnome.Sdk` runtime version "47" must be
  re-checked against `flatpak remote-info flathub org.gnome.Platform` at
  build time — GNOME ships a new numbered runtime yearly.

## 4. GNOME platform dependency versions

`gtk4`/`libadwaita`/`vte4` binding crate versions were verified against
crates.io/docs.rs on 2026-10-21 to resolve without conflict:
`gtk4 = 0.11.5`, `libadwaita = 0.9.2`, `gio = 0.22.10`, `glib = 0.22.10`,
`vte4 = 0.10.0` (the first `vte4` release built against `gtk4-sys ^0.11`,
matching `libadwaita 0.9.x`'s own requirement). The `features = ["v4_14"]`
/`["v1_5"]` flags select the **minimum** GTK4/libadwaita C library version
Nexus requires at runtime, independent of these binding crate version
numbers — keep that pair in sync with the Flatpak runtime version.

## 5. GTK/UI layer

See the module-level doc comments added by the UI implementation pass for
the authoritative, file-by-file list of what is fully wired versus left
as an application-layer TODO (e.g. provider instances are not yet
constructed/injected into the chat view — that wiring is the next piece
of work once this scaffold is reviewed). Any `// VERIFY:` comment in UI
code marks a gtk-rs/libadwaita API shape the implementer was not fully
certain of (e.g. an exact `@extends` chain or whether a given libadwaita
1.5+ widget such as `AdwToggleGroup` exists) — grep for `// VERIFY:`
before relying on that code path.

## 6. Checks to run before a real Flathub submission

1. `cargo build --release` succeeds with a real, committed `Cargo.lock`.
2. Regenerate `build-aux/flatpak/cargo-sources.json` with
   `flatpak-cargo-generator.py` against that `Cargo.lock`.
3. Change the manifest's `nexus` module source from `type: dir` to
   `type: git` with a real `url` and a `tag` pinned to a tagged release
   (never `branch: main`, per Flathub policy).
4. Rename every `GITHUB_OWNER`/`io.github.GITHUB_OWNER.Nexus` placeholder
   to the real maintainer handle and app ID, consistently across
   `Cargo.toml`, the Flatpak manifest, `data/*.in`/`*.xml`,
   `resources/nexus.gresource.xml`'s resource prefix, and the Rust source
   that embeds that prefix string.
5. Replace the metainfo's placeholder screenshot with a real one hosted
   at a stable URL.
6. Re-verify the GNOME runtime version against
   `flatpak remote-info flathub org.gnome.Platform`.
7. Run `flatpak-builder-lint manifest build-aux/flatpak/*.json` and
   `appstreamcli validate --pedantic data/*.metainfo.xml` (post-`i18n.merge_file`,
   i.e. against the built `.xml`, not the `.xml.in`) and fix anything
   that is not an expected placeholder complaint.
8. Manually test the "Run CLI agents installed on this computer" toggle
   both inside a Flatpak sandbox (confirm `flatpak-spawn --host` works
   and the confirmation dialog shows the exact command before running
   it) and with the feature left off (confirm no host process is ever
   spawned).
9. Manually test with network access revoked at the OS/firewall level to
   see what actually happens to local-model functionality (see item 3 of
   section 3 above) and update this document with the real answer.
10. Run `cargo test --workspace` and fix anything that regressed once all
    sibling crates are compiled together (each crate was verified in
    isolation during parallel development; a full-workspace run has not
    been done yet in this environment since `meson`/`flatpak-builder`
    are not installed here).
