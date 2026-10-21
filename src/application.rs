//! `NexusApplication`, the `adw::Application` subclass that owns every
//! process-wide singleton: the Tokio runtime handle, the opened
//! `nexus_store::Store`, the `GSettings` object, and the single
//! `NexusWindow`.
//!
//! Fully wired: application-level actions (`app.about`, `app.preferences`,
//! `app.shortcuts`, `app.quit`) with real handlers and accelerators;
//! `AdwAboutDialog` populated from `config.rs`; opening the on-disk SQLite
//! store at a real XDG data path the first time it is needed; constructing
//! and presenting the single `NexusWindow` on `activate`.
//!
//! Stubbed / left for the application layer to extend: this file does not
//! yet construct any `nexus_providers::Provider` or `nexus_cli_agents`
//! instances, nor does it wire `NexusWindow`'s views to real provider/CLI
//! data -- see the `// TODO` markers in `window.rs` and `src/views/*.rs`.
//! `NexusPreferences` is constructed lazily and handed the shared
//! `Arc<nexus_store::Store>` plus a real `Arc<dyn SecretStore>`
//! (`nexus_secrets::KeyringSecretStore`); wiring an actual `CommandRunner`
//! for the "launch CLI login" button is left to the application layer since
//! it depends on the `enabled-host-cli-execution` setting already read in
//! `preferences.rs`.

use std::sync::{Arc, OnceLock as SyncOnceCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::WeakRef;
use gtk::{gio, glib};

use crate::config;
use crate::window::NexusWindow;

mod imp {
    use super::*;

    pub struct NexusApplication {
        pub window: SyncOnceCell<WeakRef<NexusWindow>>,
        // `tokio::runtime::Handle` has no `Default` impl, so (unlike a
        // simple `#[derive(Default)]` struct) it is stored behind a
        // `OnceLock` set exactly once from `NexusApplication::new`, rather
        // than as a bare field as sketched in the original spec.
        pub tokio_handle: SyncOnceCell<tokio::runtime::Handle>,
        pub store: SyncOnceCell<Arc<nexus_store::Store>>,
        pub secret_store: SyncOnceCell<Arc<dyn nexus_core::SecretStore>>,
        pub settings: SyncOnceCell<gio::Settings>,
    }

    impl Default for NexusApplication {
        fn default() -> Self {
            Self {
                window: SyncOnceCell::new(),
                tokio_handle: SyncOnceCell::new(),
                store: SyncOnceCell::new(),
                secret_store: SyncOnceCell::new(),
                settings: SyncOnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NexusApplication {
        const NAME: &'static str = "NexusApplication";
        type Type = super::NexusApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for NexusApplication {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_gactions();
            obj.setup_accels();
        }
    }

    impl ApplicationImpl for NexusApplication {
        fn activate(&self) {
            self.parent_activate();
            self.obj().present_main_window();
        }

        fn startup(&self) {
            self.parent_startup();
            // `adw::Application::startup` (the parent implementation we just
            // called) already performs `adw::init()` / `gtk::init()` -- no
            // explicit re-init needed here.
        }
    }

    impl GtkApplicationImpl for NexusApplication {}
    impl AdwApplicationImpl for NexusApplication {}
}

glib::wrapper! {
    pub struct NexusApplication(ObjectSubclass<imp::NexusApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl NexusApplication {
    pub fn new(tokio_handle: tokio::runtime::Handle) -> Self {
        let app: Self = glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("flags", gio::ApplicationFlags::empty())
            .property("resource-base-path", "/io/github/GITHUB_OWNER/Nexus")
            .build();
        app.imp()
            .tokio_handle
            .set(tokio_handle)
            .expect("tokio handle is only ever set once, from NexusApplication::new");
        app
    }

    pub fn tokio_handle(&self) -> tokio::runtime::Handle {
        self.imp()
            .tokio_handle
            .get()
            .expect("tokio handle set in NexusApplication::new")
            .clone()
    }

    pub fn settings(&self) -> &gio::Settings {
        self.imp()
            .settings
            .get_or_init(|| gio::Settings::new(config::APP_ID))
    }

    pub fn secret_store(&self) -> Arc<dyn nexus_core::SecretStore> {
        self.imp()
            .secret_store
            .get_or_init(|| Arc::new(nexus_secrets::KeyringSecretStore::new()))
            .clone()
    }

    /// Returns the already-opened store, if `ensure_store_opened` has
    /// completed at least once. Views that need the store before it is
    /// guaranteed to exist should await `ensure_store_opened` first.
    pub fn store(&self) -> Option<Arc<nexus_store::Store>> {
        self.imp().store.get().cloned()
    }

    /// Opens the on-disk SQLite store (creating its parent directory and
    /// running migrations if needed) the first time it is called; later
    /// calls are a cheap no-op that returns the already-opened handle.
    ///
    /// Must be awaited from a `glib::spawn_future_local` context (GTK main
    /// thread); internally it hands the actual blocking open off to the
    /// Tokio runtime via `Handle::spawn` and awaits the resulting
    /// `JoinHandle`, which is safe to do from any thread.
    pub async fn ensure_store_opened(&self) -> Arc<nexus_store::Store> {
        if let Some(store) = self.store() {
            return store;
        }

        let data_dir = glib::user_data_dir().join("nexus");
        if let Err(err) = std::fs::create_dir_all(&data_dir) {
            tracing::error!("failed to create {}: {err}", data_dir.display());
        }
        let db_path = data_dir.join("nexus.db");

        let handle = self.tokio_handle();
        let opened = handle
            .spawn(async move { nexus_store::Store::open(&db_path).await })
            .await
            .expect("store-open task panicked")
            .expect("failed to open the local database");

        let store = Arc::new(opened);
        // `set` only fails if another call already won the race; either way
        // `self.store()` now returns a value.
        let _ = self.imp().store.set(store.clone());
        store
    }

    fn setup_gactions(&self) {
        let quit_action = gio::SimpleAction::new("quit", None);
        quit_action.connect_activate(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.quit()
        ));
        self.add_action(&quit_action);

        let about_action = gio::SimpleAction::new("about", None);
        about_action.connect_activate(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.show_about_dialog()
        ));
        self.add_action(&about_action);

        let preferences_action = gio::SimpleAction::new("preferences", None);
        preferences_action.connect_activate(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.show_preferences()
        ));
        self.add_action(&preferences_action);

        let shortcuts_action = gio::SimpleAction::new("shortcuts", None);
        shortcuts_action.connect_activate(glib::clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| app.show_shortcuts()
        ));
        self.add_action(&shortcuts_action);
    }

    fn setup_accels(&self) {
        self.set_accels_for_action("app.quit", &["<primary>q"]);
        self.set_accels_for_action("app.preferences", &["<primary>comma"]);
        self.set_accels_for_action("app.shortcuts", &["<primary>question"]);
    }

    fn main_window(&self) -> Option<NexusWindow> {
        self.imp().window.get().and_then(WeakRef::upgrade)
    }

    fn present_main_window(&self) {
        if let Some(window) = self.main_window() {
            window.present();
            return;
        }

        let window = NexusWindow::new(self);
        let weak = glib::WeakRef::new();
        weak.set(Some(&window));
        self.imp()
            .window
            .set(weak)
            .expect("main window is only ever created once per process");
        window.present();
    }

    fn show_about_dialog(&self) {
        let Some(window) = self.main_window() else {
            return;
        };

        let about = adw::AboutDialog::builder()
            .application_name("Nexus")
            .application_icon(config::APP_ID)
            .version(config::VERSION)
            .developer_name("GITHUB_OWNER")
            .license_type(gtk::License::Gpl30)
            .website("https://github.com/GITHUB_OWNER/nexus")
            .issue_url("https://github.com/GITHUB_OWNER/nexus/issues")
            .comments("Chat with AI providers and run CLI coding agents in one place.")
            .build();
        about.present(Some(&window));
    }

    fn show_preferences(&self) {
        let Some(window) = self.main_window() else {
            return;
        };
        let prefs = crate::preferences::NexusPreferences::new();
        prefs.set_settings(self.settings().clone());
        prefs.set_secret_store(self.secret_store());
        prefs.set_tokio_handle(self.tokio_handle());
        if let Some(store) = self.store() {
            prefs.set_store(store);
        }
        prefs.present(Some(&window));
    }

    // VERIFY: `Adw.ShortcutsDialog` was added in libadwaita 1.6, newer than
    // the >= 1.5 this project requires (`adw` crate gated at `v1_5` in
    // Cargo.toml) -- `gtk::ShortcutsWindow` (definitely stable since GTK4's
    // introduction) is used instead, loaded from `shortcuts.blp` via a
    // plain (non-template) `gtk::Builder` object lookup.
    fn show_shortcuts(&self) {
        let Some(window) = self.main_window() else {
            return;
        };
        let builder = gtk::Builder::from_resource("/io/github/GITHUB_OWNER/Nexus/ui/shortcuts.ui");
        let Some(shortcuts): Option<gtk::ShortcutsWindow> = builder.object("shortcuts_window")
        else {
            tracing::warn!("shortcuts.ui did not contain an object named 'shortcuts_window'");
            return;
        };
        shortcuts.set_transient_for(Some(&window));
        shortcuts.set_application(Some(self));
        shortcuts.present();
    }
}
