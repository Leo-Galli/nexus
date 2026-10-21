//! `NexusPreferences`, the Preferences dialog.
//!
//! Fully wired:
//! - "Accounts & Services": API-key providers can have a key saved (via
//!   the real `Arc<dyn SecretStore>`) or deleted; the status badge and
//!   expander content update immediately afterward. OAuth-style providers
//!   show a "Launch CLI Login" row.
//! - "Models & Defaults": the three fields are persisted through
//!   `nexus_store::settings` (there is no GSettings key for them --
//!   verified against `data/io.github.GITHUB_OWNER.Nexus.gschema.xml`,
//!   which only has `window-*`, `enabled-host-cli-execution`,
//!   `telemetry-enabled`, `pricing-table-last-updated`, and
//!   `default-chat-model-ids`).
//! - "Privacy": "Delete All Local Data" is wired to a real confirmation
//!   `AdwAlertDialog` then `nexus_store::conversations::delete_all`.
//! - "Advanced": `host_cli_execution_row` is bound directly to the real
//!   `enabled-host-cli-execution` GSettings key via `gio::Settings::bind`.
//!
//! Stubbed / left for the application layer:
//! - `launch_login_button` (OAuth providers) calls the `CommandRunner` set
//!   via `set_command_runner` with whatever `login_command` the
//!   application layer put in that provider's `ProviderSummary`; if it
//!   left `login_command` as `None` (the default, since this view has no
//!   knowledge of any real provider's actual CLI login invocation), the
//!   button shows a toast explaining nothing is configured yet rather than
//!   silently doing nothing. // TODO: the application layer should fill in
//!   real `login_command`s once it knows which OAuth-backed providers ship
//!   a documented non-interactive login invocation.
//! - Secret-store and store calls are async; since `NexusPreferences` is
//!   constructed before any `tokio::runtime::Handle` is necessarily known,
//!   callers MUST call `set_tokio_handle` (in addition to `set_secret_store`/
//!   `set_store`) before those buttons will do anything -- `NexusApplication`
//!   does this in `show_preferences`.

use std::cell::RefCell;
use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};
use nexus_core::{AuthMethod, CommandRunner, ProviderStatus, SecretKey, SecretStore};

/// Everything `NexusPreferences` needs to render one provider row. No
/// `Arc<dyn Provider>` is held here -- only the metadata needed to
/// display status and build a `SecretKey`, mirroring the read-only
/// `CliAgentInfo`-style pattern used by `agents_view`.
#[derive(Clone)]
pub struct ProviderSummary {
    pub secret_key: SecretKey,
    pub display_name: String,
    pub auth_method: AuthMethod,
    pub api_key_url: Option<&'static str>,
    pub status: ProviderStatus,
    /// `None` until the application layer knows a real non-interactive
    /// login invocation for this (OAuth-backed) provider.
    pub login_command: Option<(String, Vec<String>)>,
}

// ---------------------------------------------------------------------
// NexusProviderRow
// ---------------------------------------------------------------------

mod row_imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/provider-row.ui")]
    pub struct ProviderRow {
        #[template_child]
        pub status_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub key_entry_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub key_actions_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub open_key_page_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_key_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub disconnect_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub oauth_actions_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub launch_login_button: TemplateChild<gtk::Button>,
        pub summary: RefCell<Option<ProviderSummary>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProviderRow {
        const NAME: &'static str = "NexusProviderRow";
        type Type = super::ProviderRow;
        type ParentType = adw::ExpanderRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ProviderRow {}
    impl WidgetImpl for ProviderRow {}
    impl ListBoxRowImpl for ProviderRow {}
    impl PreferencesRowImpl for ProviderRow {}
    impl ExpanderRowImpl for ProviderRow {}
}

glib::wrapper! {
    pub struct ProviderRow(ObjectSubclass<row_imp::ProviderRow>)
        @extends gtk::Widget, gtk::ListBoxRow, adw::PreferencesRow, adw::ExpanderRow,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl ProviderRow {
    pub fn new(summary: ProviderSummary) -> Self {
        let row: Self = glib::Object::builder().build();
        row.set_title(&summary.display_name);
        row.apply_summary(&summary);
        *row.imp().summary.borrow_mut() = Some(summary);
        row
    }

    pub fn summary(&self) -> Option<ProviderSummary> {
        self.imp().summary.borrow().clone()
    }

    fn apply_summary(&self, summary: &ProviderSummary) {
        let imp = self.imp();
        imp.status_badge.set_text(if summary.status.configured {
            "Connected"
        } else {
            "Not configured"
        });

        match summary.auth_method {
            AuthMethod::ApiKey => {
                imp.key_entry_row.set_visible(true);
                imp.key_actions_row.set_visible(true);
                imp.oauth_actions_row.set_visible(false);
                imp.open_key_page_button.set_visible(summary.api_key_url.is_some());
                imp.disconnect_button.set_visible(summary.status.configured);
                if summary.status.configured {
                    imp.key_entry_row.set_text("");
                    imp.key_entry_row
                        .set_tooltip_text(Some("A key is already stored; enter a new one to replace it."));
                }
            }
            AuthMethod::OAuth => {
                imp.key_entry_row.set_visible(false);
                imp.key_actions_row.set_visible(false);
                imp.oauth_actions_row.set_visible(true);
            }
            AuthMethod::None => {
                imp.key_entry_row.set_visible(false);
                imp.key_actions_row.set_visible(false);
                imp.oauth_actions_row.set_visible(false);
            }
        }
    }
}

// ---------------------------------------------------------------------
// NexusPreferences
// ---------------------------------------------------------------------

mod imp {
    use super::*;
    use std::cell::OnceCell;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/preferences.ui")]
    pub struct NexusPreferences {
        #[template_child]
        pub providers_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub default_temperature_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub default_max_tokens_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub default_system_prompt_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub delete_all_data_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub host_cli_execution_row: TemplateChild<adw::SwitchRow>,

        pub settings: OnceCell<gio::Settings>,
        pub secret_store: OnceCell<Arc<dyn SecretStore>>,
        pub command_runner: OnceCell<Arc<dyn CommandRunner>>,
        pub store: OnceCell<Arc<nexus_store::Store>>,
        pub tokio_handle: OnceCell<tokio::runtime::Handle>,
    }

    impl Default for NexusPreferences {
        fn default() -> Self {
            Self {
                providers_list: TemplateChild::default(),
                default_temperature_row: TemplateChild::default(),
                default_max_tokens_row: TemplateChild::default(),
                default_system_prompt_row: TemplateChild::default(),
                delete_all_data_button: TemplateChild::default(),
                host_cli_execution_row: TemplateChild::default(),
                settings: OnceCell::new(),
                secret_store: OnceCell::new(),
                command_runner: OnceCell::new(),
                store: OnceCell::new(),
                tokio_handle: OnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NexusPreferences {
        const NAME: &'static str = "NexusPreferences";
        type Type = super::NexusPreferences;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NexusPreferences {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_signals();
        }
    }
    impl WidgetImpl for NexusPreferences {}
    // VERIFY: `AdwDialogImpl`/`PreferencesDialogImpl` are the trait names
    // libadwaita-rs 0.9.x generates for subclassing `Adw.PreferencesDialog`;
    // neither has required methods for a dialog that only uses the
    // template's static pages.
    impl adw::subclass::prelude::AdwDialogImpl for NexusPreferences {}
    impl adw::subclass::prelude::PreferencesDialogImpl for NexusPreferences {}
}

glib::wrapper! {
    pub struct NexusPreferences(ObjectSubclass<imp::NexusPreferences>)
        @extends gtk::Widget, adw::Dialog, adw::PreferencesDialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl Default for NexusPreferences {
    fn default() -> Self {
        Self::new()
    }
}

impl NexusPreferences {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_settings(&self, settings: gio::Settings) {
        let switch_row = self.imp().host_cli_execution_row.get();
        settings
            .bind("enabled-host-cli-execution", &switch_row, "active")
            .build();
        let _ = self.imp().settings.set(settings);
    }

    pub fn set_secret_store(&self, store: Arc<dyn SecretStore>) {
        let _ = self.imp().secret_store.set(store);
    }

    pub fn set_command_runner(&self, runner: Arc<dyn CommandRunner>) {
        let _ = self.imp().command_runner.set(runner);
    }

    pub fn set_tokio_handle(&self, handle: tokio::runtime::Handle) {
        let _ = self.imp().tokio_handle.set(handle);
    }

    pub fn set_store(&self, store: Arc<nexus_store::Store>) {
        let _ = self.imp().store.set(store.clone());
        self.load_model_defaults(store);
    }

    pub fn set_providers(&self, providers: Vec<ProviderSummary>) {
        let imp = self.imp();
        while let Some(row) = imp.providers_list.row_at_index(0) {
            imp.providers_list.remove(&row);
        }
        for summary in providers {
            let row = ProviderRow::new(summary);
            self.wire_provider_row(&row);
            imp.providers_list.append(&row);
        }
    }

    fn wire_provider_row(&self, row: &ProviderRow) {
        let row_imp = row.imp();

        row_imp.save_key_button.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            #[weak]
            row,
            move |_| prefs.save_api_key(&row)
        ));
        row_imp.disconnect_button.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            #[weak]
            row,
            move |_| prefs.disconnect_provider(&row)
        ));
        row_imp.open_key_page_button.connect_clicked(glib::clone!(
            #[weak]
            row,
            move |_| {
                if let Some(summary) = row.summary() {
                    if let Some(url) = summary.api_key_url {
                        gtk::UriLauncher::new(url).launch(
                            None::<&gtk::Window>,
                            gio::Cancellable::NONE,
                            |_| {},
                        );
                    }
                }
            }
        ));
        row_imp.launch_login_button.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            #[weak]
            row,
            move |_| prefs.launch_login(&row)
        ));
    }

    fn save_api_key(&self, row: &ProviderRow) {
        let Some(summary) = row.summary() else { return };
        let text = row.imp().key_entry_row.text().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some(secret_store) = self.imp().secret_store.get().cloned() else {
            tracing::warn!("save_api_key: no SecretStore configured");
            return;
        };
        let Some(handle) = self.imp().tokio_handle.get().cloned() else {
            tracing::warn!("save_api_key: no tokio handle configured");
            return;
        };
        let key = summary.secret_key.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak]
            row,
            async move {
                let result = handle
                    .spawn(async move { secret_store.set(&key, &text).await })
                    .await
                    .expect("secret-store task panicked");
                match result {
                    Ok(()) => {
                        if let Some(mut summary) = row.summary() {
                            summary.status.configured = true;
                            row.apply_summary(&summary);
                            *row.imp().summary.borrow_mut() = Some(summary);
                        }
                    }
                    Err(err) => tracing::error!("failed to save API key: {err}"),
                }
            }
        ));
    }

    fn disconnect_provider(&self, row: &ProviderRow) {
        let Some(summary) = row.summary() else { return };
        let Some(secret_store) = self.imp().secret_store.get().cloned() else {
            return;
        };
        let Some(handle) = self.imp().tokio_handle.get().cloned() else {
            return;
        };
        let key = summary.secret_key.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak]
            row,
            async move {
                let result = handle
                    .spawn(async move { secret_store.delete(&key).await })
                    .await
                    .expect("secret-store task panicked");
                match result {
                    Ok(()) => {
                        if let Some(mut summary) = row.summary() {
                            summary.status.configured = false;
                            row.apply_summary(&summary);
                            *row.imp().summary.borrow_mut() = Some(summary);
                        }
                    }
                    Err(err) => tracing::error!("failed to delete API key: {err}"),
                }
            }
        ));
    }

    fn launch_login(&self, row: &ProviderRow) {
        let Some(summary) = row.summary() else { return };
        let Some((program, args)) = summary.login_command else {
            self.show_toast_if_possible(&format!(
                "No CLI login command is configured yet for {}.",
                summary.display_name
            ));
            return;
        };
        let Some(runner) = self.imp().command_runner.get().cloned() else {
            return;
        };
        let Some(handle) = self.imp().tokio_handle.get().cloned() else {
            return;
        };
        glib::spawn_future_local(async move {
            let result = handle
                .spawn(async move { runner.run(&program, &args, None).await })
                .await
                .expect("command-runner task panicked");
            if let Err(err) = result {
                tracing::error!("failed to launch CLI login: {err}");
            }
        });
    }

    fn load_model_defaults(&self, store: Arc<nexus_store::Store>) {
        let Some(handle) = self.imp().tokio_handle.get().cloned() else {
            return;
        };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            async move {
                let store_for_task = store.clone();
                let (temperature, max_tokens, system_prompt) = handle
                    .spawn(async move {
                        (
                            nexus_store::settings::get(&store_for_task, "default_temperature")
                                .await
                                .ok()
                                .flatten(),
                            nexus_store::settings::get(&store_for_task, "default_max_tokens")
                                .await
                                .ok()
                                .flatten(),
                            nexus_store::settings::get(&store_for_task, "default_system_prompt")
                                .await
                                .ok()
                                .flatten(),
                        )
                    })
                    .await
                    .expect("settings-load task panicked");

                let imp = prefs.imp();
                if let Some(t) = temperature.and_then(|s| s.parse::<f64>().ok()) {
                    imp.default_temperature_row.set_value(t);
                }
                if let Some(m) = max_tokens.and_then(|s| s.parse::<f64>().ok()) {
                    imp.default_max_tokens_row.set_value(m);
                }
                if let Some(p) = system_prompt {
                    imp.default_system_prompt_row.set_text(&p);
                }
            }
        ));
    }

    fn persist_setting(&self, key: &'static str, value: String) {
        let Some(store) = self.imp().store.get().cloned() else {
            return;
        };
        let Some(handle) = self.imp().tokio_handle.get().cloned() else {
            return;
        };
        glib::spawn_future_local(async move {
            let _ = handle
                .spawn(async move { nexus_store::settings::set(&store, key, &value).await })
                .await;
        });
    }

    fn setup_signals(&self) {
        let imp = self.imp();

        imp.default_temperature_row.connect_value_notify(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |row| prefs.persist_setting("default_temperature", row.value().to_string())
        ));
        imp.default_max_tokens_row.connect_value_notify(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |row| prefs.persist_setting("default_max_tokens", row.value().to_string())
        ));
        imp.default_system_prompt_row.connect_changed(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |row| prefs.persist_setting("default_system_prompt", row.text().to_string())
        ));

        imp.delete_all_data_button.connect_clicked(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            move |_| prefs.confirm_delete_all_data()
        ));
    }

    fn confirm_delete_all_data(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading("Delete All Local Data?")
            .body("This permanently deletes every conversation and message stored on this device. This cannot be undone.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = prefs)]
                self,
                move |_, response| {
                    if response == "delete" {
                        prefs.delete_all_data();
                    }
                }
            ),
        );
        dialog.present(Some(self));
    }

    fn delete_all_data(&self) {
        let Some(store) = self.imp().store.get().cloned() else {
            return;
        };
        let Some(handle) = self.imp().tokio_handle.get().cloned() else {
            return;
        };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = prefs)]
            self,
            async move {
                let result = handle
                    .spawn(async move { nexus_store::conversations::delete_all(&store).await })
                    .await
                    .expect("delete-all task panicked");
                match result {
                    Ok(()) => prefs.show_toast_if_possible("All local data was deleted."),
                    Err(err) => tracing::error!("failed to delete local data: {err}"),
                }
            }
        ));
    }

    // VERIFY: `Adw.PreferencesDialog::add_toast` (a built-in toast overlay
    // convenience, mirroring the older `Adw.PreferencesWindow::add_toast`)
    // against libadwaita-rs 0.9.x's actual generated API.
    fn show_toast_if_possible(&self, text: &str) {
        self.add_toast(adw::Toast::new(text));
    }
}
