//! The four top-level views hosted by `NexusWindow`'s content stack, plus
//! the shared `EmptyState` widget they all use for "nothing here yet"
//! placeholders.

pub mod agents_view;
pub mod automations_view;
pub mod chat_view;
pub mod stats_view;

/// Builds a "nothing here yet" `Adw.StatusPage` from
/// `resources/ui/empty-state.blp`'s compiled template
/// (`ui/empty-state.ui`, listed in `nexus.gresource.xml`).
///
/// This is a plain `gtk::Builder` object lookup rather than a
/// composite-template subclass: libadwaita-rs 0.9.2 does not expose a
/// `StatusPageImpl` subclassing trait for `Adw.StatusPage` (confirmed by
/// checking its `subclass` module, which has no `status_page.rs`), so
/// there is no way to subclass it the way every other widget in this
/// crate is subclassed. A plain, freshly-built `Adw.StatusPage` is
/// returned instead, with `icon-name`/`title`/`description` set directly.
pub fn new_empty_state(icon_name: &str, title: &str, description: &str) -> adw::StatusPage {
    let builder =
        gtk::Builder::from_resource("/io/github/GITHUB_OWNER/Nexus/ui/empty-state.ui");
    let status_page: adw::StatusPage = builder
        .object("empty_state")
        .expect("empty-state.ui must define an object named 'empty_state'");
    status_page.set_icon_name(Some(icon_name));
    status_page.set_title(title);
    status_page.set_description(Some(description));
    status_page
}
