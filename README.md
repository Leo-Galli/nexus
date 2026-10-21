# Nexus

Nexus is a GNOME desktop application that brings together chat access to
multiple AI providers, local models, and command-line coding agents in
one place: compare answers from several models side by side, drive
coding-agent CLIs from an integrated terminal, build multi-step
automations, and keep a fully local record of usage and cost.

"Nexus" is a provisional project name and `io.github.GITHUB_OWNER.Nexus`
a provisional application ID — see `docs/FLATHUB_SUBMISSION.md` step 0
before a real release.

## Status

Early-stage, under active development. See `docs/LIMITATIONS.md` for an
honest list of what is a full implementation versus a documented stub,
and what has not yet been verified against a real end-to-end build.

## Features (target scope)

- **Chat**: converse with one or several models at once, streamed,
  Markdown-rendered with code highlighting, searchable history, folders
  and favorites, export to Markdown/JSON.
- **CLI agents**: drive Claude Code, OpenAI Codex CLI, Gemini CLI, Qwen
  Code, Cursor, Aider, GitHub Copilot CLI (and any other CLI you
  configure by hand) from an integrated terminal or a structured view,
  with working-directory selection via the file portal and confirmation
  before file-touching actions.
- **Automations**: multi-step flows (prompt a model, run a CLI command,
  transform text, branch on a condition, send a notification, write a
  file) with manual, scheduled, or folder-watch triggers, and per-step
  fallback to another model on error.
- **Model management**: favorites, aliases, per-model default parameters,
  profiles.
- **Statistics**: requests, tokens, estimated cost (from a hand-maintained
  pricing table — never guessed), latency, and error rate, broken down by
  day/week/month, exportable to CSV. Everything stays on your device.
- **Privacy**: zero telemetry, data stored locally in SQLite, API keys
  held only in the system secret store (Secret Service / Secret portal),
  never in a plaintext file.

## Architecture

See `ARCHITECTURE.md` for the component diagram, the async execution
model (Tokio + the GLib main loop), and the data flow for a chat request.
See `docs/CONTRACTS.md` for the frozen inter-crate API surface.

## Repository layout

```
crates/
  nexus-core/         Provider & CliAgent traits, shared types, errors
  nexus-secrets/       Secret storage (oo7 / Secret Service / portal)
  nexus-store/         SQLite-backed persistence (rusqlite)
  nexus-providers/     One module per AI provider
  nexus-cli-agents/    One module per CLI coding agent
  nexus-automation/    Step engine, triggers, run log
  nexus-stats/         Usage aggregation, pricing table, CSV export
src/                   GTK4 + libadwaita application (binary `nexus`)
resources/             Blueprint UI sources + GResource manifest
data/                  Desktop entry, AppStream metainfo, icons, GSettings
build-aux/flatpak/      Flatpak manifest, permission justifications
po/                     Translations (gettext)
docs/                   Build, Flathub submission, and limitations docs
```

## Building

See `docs/BUILDING.md` for GNOME Builder, manual `flatpak-builder`, and
plain-`cargo` development workflows.

## Contributing to Flathub

See `docs/FLATHUB_SUBMISSION.md`.

## License

GPL-3.0-or-later. See `LICENSE`.
