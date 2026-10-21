# Nexus

A GNOME desktop application, built with Rust, GTK4 and libadwaita, that
brings together chat access to multiple AI providers, local models, and
command-line coding agents in one place — with multi-model comparison,
automations, and fully local usage statistics.

**Status: work in progress scaffold.** Core traits, provider
implementations, CLI agent integrations, the automation engine, the local
SQLite store, and Flatpak packaging are implemented and unit-tested
per-crate (see `docs/LIMITATIONS.md`); the GTK4/libadwaita UI and the
wiring between the UI and the backend crates are still being built out.
This is not yet installable from Flathub.

- **License:** GPL-3.0-or-later
- **App ID (placeholder):** `io.github.GITHUB_OWNER.Nexus` — every
  occurrence of `GITHUB_OWNER` across this repository is a literal
  placeholder to be renamed by whoever publishes this app, in one pass,
  before a real release (see `docs/LIMITATIONS.md` checklist).

## Documentation

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — component diagram, async
  execution model, crate layout.
- [`docs/CONTRACTS.md`](docs/CONTRACTS.md) — the frozen `nexus-store`
  public API other crates are built against.
- [`docs/BUILDING.md`](docs/BUILDING.md) — building with GNOME Builder,
  `meson`/`ninja`, and `flatpak-builder`; the Flathub submission process.
- [`docs/LIMITATIONS.md`](docs/LIMITATIONS.md) — what is implemented,
  what is a documented stub, every unverified API/flag detail, and the
  pre-submission checklist.
- [`build-aux/flatpak/PERMISSIONS.md`](build-aux/flatpak/PERMISSIONS.md)
  — justification for every Flatpak sandbox permission Nexus requests.

## Repository layout

```
crates/
  nexus-core/        Provider & CliAgent traits, shared types, errors
  nexus-secrets/      Secret Service / portal-backed credential storage
  nexus-store/        SQLite persistence (conversations, automations, usage)
  nexus-providers/    One module per AI provider implementation
  nexus-cli-agents/   One module per coding-agent CLI integration
  nexus-automation/   Step engine, triggers, run log
  nexus-stats/        Usage aggregation, pricing table, CSV export
src/                  GTK4/libadwaita application (binary crate `nexus`)
resources/            Blueprint UI sources, GResource manifest, CSS
data/                 Desktop entry, AppStream metainfo, icons, GSettings schema
build-aux/flatpak/    Flatpak manifest and permission justifications
po/                   gettext translations
docs/                 Architecture, build, and limitations documentation
```

## Quick start (development build)

```sh
meson setup _build
ninja -C _build
_build/src/nexus
```

See `docs/BUILDING.md` for prerequisites, the GNOME Builder workflow, and
the full Flatpak build/Flathub submission process.

## Contributing

This project is still early scaffolding. Before adding a new provider or
CLI agent integration, read `docs/LIMITATIONS.md` for the house rule this
codebase follows strictly: never invent an API endpoint, model name, or
CLI flag — if it isn't verified against official documentation, mark it
`UNVERIFIED` in a doc comment and degrade gracefully instead of guessing.
