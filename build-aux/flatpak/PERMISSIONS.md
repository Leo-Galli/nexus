# Nexus Flatpak permissions (`finish-args`) — justification

This document exists to be adapted directly into Flathub pull-request review
responses. Every `finish-args` entry in
`build-aux/flatpak/io.github.GITHUB_OWNER.Nexus.json` is justified below:
what it is for, why it is the minimum required, and what happens inside
Nexus when the permission is absent or denied. Anywhere this document is not
fully certain of a technical claim, it says so explicitly with an
`UNCERTAIN` marker rather than asserting it.

Top-level note on scope: this file covers only the Flatpak sandbox
permissions declared in the manifest's `finish-args`. It does not cover
Flathub's `flathub.json` repository-configuration keys in depth (see below)
or anything in `docs/LIMITATIONS.md` (owned elsewhere).

### Note on `build-aux/flatpak/flathub.json`

`flathub.json` only includes the `only-arches` key, listing `x86_64` and
`aarch64` — the two architectures Nexus is expected to build and run on
given its Rust/GTK4/libadwaita stack, which has no known arch-specific
blockers. This is believed to be a genuine, documented Flathub repository-
config key. `UNCERTAIN`: Flathub's `flathub.json` is also commonly
documented (in various Flathub app repositories and docs over time) to
support additional keys such as `skip-icons-check`,
`skip-appstream-check`/validation-skip flags, `exclude-arches`, and
end-of-life metadata (`end-of-life` / `end-of-life-rebase`). None of those
were added here because their exact current names, required format, and
continued support were not independently re-verified in this environment.
Before submission, check the current Flathub documentation
(`https://docs.flathub.org/`) for the authoritative, up-to-date key list
rather than relying on this file as exhaustive.

## `--share=ipc`

Standard GTK/GNOME requirement for the X11/Wayland client libraries and
shared-memory rendering paths used by GTK4. Without it, GTK's own shared
memory setup can fail or degrade on some display stacks. This is one of the
conventional baseline flags recommended for any GTK application manifest
(alongside the two `--socket=` entries below) and is not specific to any
Nexus feature. There is no reasonable fallback if denied: GTK4 apps are
generally shipped with this flag and it is not expected to be contested in
review.

## `--socket=wayland`

Lets Nexus connect to the Wayland compositor so the GTK4/libadwaita UI can
be displayed at all under native Wayland sessions. This is the primary,
preferred display path on modern GNOME. Without it, Nexus has no window
system to draw into on a Wayland-only session and cannot start a visible UI.

## `--socket=fallback-x11`

Lets Nexus fall back to X11 (via Xwayland or a genuine X11 session) when
Wayland is unavailable. `fallback-x11` (rather than the older unconditional
`--socket=x11`) is the minimal, recommended form: it grants X11 access only
when Wayland access was not actually usable, which is the documented
Flatpak-recommended pattern for GTK apps that primarily target Wayland. If
denied, Nexus cannot display a UI on X11-only sessions; Wayland sessions are
unaffected.

## `--device=dri`

Grants access to the DRI/GPU device nodes so GTK4's renderer (GSK, via
OpenGL/Vulkan) can use hardware acceleration. Without it, GTK4 typically
falls back to a software (cairo/llvmpipe) rendering path, which still works
but is slower and more CPU-intensive, particularly noticeable in the chat
view's streaming text updates and any animated libadwaita widgets. Denying
this does not break Nexus, it only degrades rendering performance.

## `--share=network`

Required for Nexus's core function: sending chat requests to configured AI
provider APIs (OpenAI, Anthropic, Google Gemini, Cohere, Azure OpenAI,
OpenAI-compatible endpoints, etc.) over HTTPS, and for discovering/talking
to **local** model servers (Ollama, LM Studio, llama.cpp server) which, even
when "local," are reached over loopback TCP (typically `127.0.0.1` on a
configurable port), not a filesystem or IPC mechanism.

Important accuracy note: Nexus does **not** have a mode that works with
network fully denied, including "local-only" use. Loopback HTTP to a
locally running model server is still a network connection from the
sandboxed process's point of view, and `--share=network` is Flatpak's only
sandbox control over whether the app has a network namespace at all —
Flatpak does not offer a narrower "loopback only" grant. `UNCERTAIN`: this
document has not independently re-verified, inside this environment, that a
denied `--share=network` definitely blocks 127.0.0.1 connections under every
Flatpak portal/network-namespace configuration (there is some possibility a
given sandbox setup still permits loopback without the shared network
namespace); if a reviewer or packager needs a precise answer, verify against
current Flatpak sandbox documentation/behavior before asserting it either
way. What is certain is that Nexus's remote-provider chat features need
`--share=network` with no exception, and that local-model support should
not be advertised as working without it.

If network access is denied entirely, Nexus should (and is expected to)
present an `AdwStatusPage` in the chat view explaining that no model
providers (remote or local) are reachable, rather than silently failing or
crashing. Automations whose steps call out to a provider or local model
would fail at that step and record the failure in the run log.

## `--talk-name=org.freedesktop.portal.Secret`

Lets Nexus store and retrieve provider API keys and other credentials
through the Secret portal (`org.freedesktop.portal.Secret`), which is what
the `oo7` crate uses as its sandboxed-friendly backend to the user's
Secret Service / keyring (e.g. GNOME Keyring, KWallet). This is the minimum
D-Bus name needed for secret storage under sandboxing; Nexus does not
request any broader portal access (for example, no direct, non-portal
Secret Service bus name, and no filesystem-based credential storage).

If denied, credential storage would fail: Nexus would not be able to save
or load API keys between sessions. The expected in-app behavior is that the
accounts/services preferences page shows an error state for the affected
provider and the chat view treats that provider as unconfigured, rather
than falling back to storing secrets in plaintext on disk. (Nexus does not
implement a plaintext-file fallback for secrets, by design.)

## File access for attachments and automation file steps — NOT a static `--talk-name` or `--filesystem` grant

Nexus's chat attachments feature (attaching a file to a message) and its
automation "write to file" / "watch folder" steps use GTK4's `GtkFileDialog`
and the corresponding `Gio.File` portal-backed APIs. Under a sandboxed GTK4
app, these dialogs are automatically routed through the Document portal
(`org.freedesktop.portal.Documents`) and the file chooser portal by GTK
itself; a sandboxed app does not need to separately declare
`--talk-name=org.freedesktop.portal.Documents` (or any other portal
talk-name) to use `GtkFileDialog` — every Flatpak app already has default
access to the session bus and the portal bus names needed to invoke a
portal request and be handed back a document-portal-scoped file handle for
exactly the file(s) the user picked.

Nexus's manifest therefore deliberately does **not** include
`--talk-name=org.freedesktop.portal.Documents` and does **not** include any
`--filesystem=host` (or `--filesystem=home`) grant. Per-use folder access
for the "watch folder" automation trigger and "write to file" automation
step is intended to be scoped through portal folder-selection dialogs (i.e.
the user explicitly picks a folder each time access is needed, or grants a
persisted portal permission for that specific folder via the standard
portal consent flow), not through a static, always-on filesystem grant in
the manifest. This is the more restrictive, more Flathub-review-friendly
choice; the tradeoff is that automation folder-watch cannot silently access
arbitrary paths the user has not explicitly granted through a portal
dialog.

`UNCERTAIN`: this document has not independently tested, inside this
environment, the exact behavior of a cron-like/unattended automation
trigger (i.e., folder-watch running with no user present to drive a dialog)
against portal-only file access — portal file access is fundamentally
interactive/consent-based, and a headless re-trigger of a previously granted
folder permission should work via the Document portal's persisted
permissions, but this has not been verified against real portal behavior in
this environment. Verify this specific interaction (persisted folder
permission re-used by an unattended trigger) before asserting it works in a
Flathub review response.

## Notifications — intentionally no `--talk-name=org.freedesktop.Notifications`

The host D-Bus name `org.freedesktop.Notifications` is the legacy mechanism
used by non-sandboxed Linux applications to show desktop notifications
directly. Sandboxed Flatpak apps are expected to use `Gio.Notification`
(via `GApplication`/`GNotification`), which is routed automatically through
the Notification portal (`org.freedesktop.portal.Notification`) without
requiring any explicit `--talk-name` entry in `finish-args` — this portal,
like the basic session-bus/portal access, is available to every sandboxed
app by default. Nexus's manifest therefore does not and should not include
`--talk-name=org.freedesktop.Notifications`; doing so would be requesting
unsandboxed-style access that a reviewer would likely flag as unnecessary
and potentially reject.

`UNCERTAIN`: this is based on general, well-documented Flatpak/portal
conventions (this is the standard, widely documented pattern for sandboxed
GNOME apps) rather than a fresh verification against the current portal
documentation in this environment. If Nexus's notification code ever needs
features beyond what `Gio.Notification` exposes (e.g. rich actions not
supported by the portal), re-check whether an explicit grant becomes
necessary at that point — but for the notification use cases described in
this spec (e.g. automation run completion/failure notices), no extra
`finish-args` entry should be needed.

## `--talk-name=org.freedesktop.Flatpak` — the CLI-agent-on-host permission (most scrutinized)

This is expected to be the single most scrutinized permission in a Flathub
review of Nexus, so this section explains the reasoning in full.

**What it is for.** Nexus's CLI-agent feature lets users invoke coding
agents that are installed on the host system (e.g. Claude Code, Codex CLI,
Gemini CLI, Qwen Code, cursor-agent, Windsurf, Antigravity, Aider, GitHub
Copilot CLI, or a user-defined generic CLI) from inside the sandboxed app.
Those binaries live on the host, not inside the Flatpak runtime, and a
sandboxed process cannot exec an arbitrary host binary directly. The
standard, documented Flatpak mechanism for a sandboxed app to run a command
on the host is `flatpak-spawn --host`, and using it requires the sandboxed
app to hold `--talk-name=org.freedesktop.Flatpak` (the D-Bus name of the
`flatpak-portal`/session-helper service that `flatpak-spawn` talks to).

**Why it must be declared statically, not toggled at the permission
level.** Flatpak has no concept of an in-app, user-togglable D-Bus
permission: `finish-args` are enforced statically at install time by the
sandbox, and there is no supported way to grant or revoke a `--talk-name`
from inside the running application. A permission in vanilla Flatpak either
is declared in the manifest (and therefore is always available to the
process, for the lifetime of that install) or it is not declared at all (and
the feature is categorically impossible, with no code path that could ever
exercise it). There is no third option where the manifest says "ask the
user first." Because of this, if Nexus is to offer the host-CLI-agent
feature at all, `--talk-name=org.freedesktop.Flatpak` must be present in
`finish-args` from the start — it cannot be added dynamically, requested
on demand via a portal-style consent dialog, or deferred to a plugin/
extension that the user installs separately.

**How Nexus still keeps this opt-in in practice.** Since the sandbox-level
permission cannot itself be made conditional, Nexus pushes the opt-in
boundary into the application layer instead: the permission is present at
install time (so it is visible, declared, and justified to Flathub
reviewers and to anyone inspecting the Flatpak's permissions, e.g. via
Flatseal), but **Nexus's own code never calls `flatpak-spawn --host` unless
the user has explicitly turned the feature on** in Preferences. This is
implemented via the `enabled-host-cli-execution` GSettings key (see
`data/io.github.GITHUB_OWNER.Nexus.gschema.xml`), which defaults to
`false`. With the setting at its default, Nexus's `CommandRunner`
(`nexus-cli-agents`, see `ARCHITECTURE.md`) never attempts a host spawn, and
the CLI-agent UI shows a disabled/explanatory state (per the architecture
diagram's "Feature disabled: status page explains why" path) rather than a
D-Bus error. Only when the user flips the preference does the runner begin
issuing `flatpak-spawn --host` calls.

**Why this is the minimum needed for the feature to exist at all.** There is
no narrower D-Bus name or portal that grants "run this one specific host
binary" without granting general host-command-execution capability through
`flatpak-spawn`; `org.freedesktop.Flatpak` is the one name that mechanism
depends on, and it is inherently coarse-grained (it does not let Flatpak
itself distinguish "run `claude`" from "run `rm -rf ~`" — that distinction
exists only in Nexus's own code, by construction of which commands the
CLI-agent detector/runner choose to issue). This is a real, inherent
limitation of the `flatpak-spawn --host` mechanism, not something unique to
Nexus's implementation of it.

**What breaks if a reviewer insists this be removed entirely.** If
`--talk-name=org.freedesktop.Flatpak` were removed from the manifest, the
host-CLI-agent feature would be completely unimplementable while sandboxed
— not degraded, but absent — since the sandbox would refuse the D-Bus call
outright regardless of any in-app setting. Nexus's documented position
(for review purposes) is that the permission is justified because: (1) it
is off by default, both at first run and persistently until the user
explicitly opts in; (2) the feature it enables is a core, clearly-described
part of the product (CLI coding agent integration is listed as a primary
feature, not a hidden extra); (3) no alternative Flatpak-sandboxed mechanism
exists for invoking arbitrary host-installed developer tools; and (4) the
blast radius of the permission is scoped by Nexus's own code paths, which
only ever invoke the specific, user-configured agent binaries the detector
found on the host PATH (or a user-specified generic CLI command), not
arbitrary commands typed by a remote or untrusted party.

`UNCERTAIN`: whether Flathub reviewers will accept this justification is a
review-outcome judgment call, not a technical fact — this document states
the technical mechanism accurately (no togglable D-Bus permission exists in
vanilla Flatpak) but cannot predict reviewer sentiment. Be prepared for a
reviewer to ask for additional UI safeguards (e.g. a one-time confirmation
dialog shown the moment the toggle is flipped, explicit warning text in
Preferences) beyond the default-off GSetting; none of that is precluded by
anything here, it is just not yet specified as implemented.

## Summary table

| `finish-args` entry | Present? | Default-on? |
|---|---|---|
| `--share=ipc` | yes | n/a (baseline) |
| `--socket=wayland` | yes | n/a (baseline) |
| `--socket=fallback-x11` | yes | n/a (baseline) |
| `--device=dri` | yes | n/a (baseline, perf only) |
| `--share=network` | yes | always needed, no local-only mode |
| `--talk-name=org.freedesktop.portal.Secret` | yes | always needed for credential storage |
| `--talk-name=org.freedesktop.portal.Documents` | **no** (handled implicitly by `GtkFileDialog`/portal) | n/a |
| `--talk-name=org.freedesktop.Notifications` | **no** (wrong mechanism for sandboxed apps; `Gio.Notification` used instead) | n/a |
| `--filesystem=host` / `--filesystem=home` | **no** (deliberately omitted) | n/a |
| `--talk-name=org.freedesktop.Flatpak` | yes, statically | feature gated off by default via `enabled-host-cli-execution` GSetting |
