//! The multi-model chat comparison view.
//!
//! Fully wired (works standalone, driven by whatever calls these methods):
//! - Adding/removing "active" models as removable chips, each opening its
//!   own scrollable message column (`add_active_model`/`remove_active_model`).
//! - The model-picker popover, populated from whatever
//!   `set_available_models` was last called with.
//! - `send_message`: appends the user's text as a local-echo message row to
//!   every active column immediately, clears the composer, and emits the
//!   `send-requested` signal.
//! - `append_delta` / `finish_stream` / `report_error`: drive a column's
//!   current assistant message from an external streaming source.
//! - The composer's attach-file button (`gtk::FileDialog`, portal-safe) and
//!   the stop button's sensitivity, tied to whether any column is currently
//!   streaming.
//!
//! Stubbed / left for the application layer:
//! - `set_available_models` only stores data for the popover to show; no
//!   default call ever populates it from here. The application layer calls
//!   it once providers are loaded.
//! - `connect_send_requested` lets the application layer build a
//!   `nexus_core::ChatRequest` per active model and actually call
//!   `Provider::send_chat` // TODO: wire to real Provider instances via
//!   ExecutionContext/registry once the application layer constructs them.
//!   This view has no network/provider/store access of its own.
//! - Markdown rendering: content is shown as escaped plain text in a
//!   `GtkLabel` (see `chat-message-row.blp`'s doc comment) -- real Markdown
//!   rendering is a follow-up needing a dependency not yet in `Cargo.toml`.
//! - File attachments: `attach_button` lets the user pick a file and shows
//!   its name next to the composer, but `nexus_core::ChatRequest` has no
//!   attachment field yet, so the picked file is not actually sent anywhere
//!   -- this is UI plumbing only, ready for the application layer to extend
//!   once attachments are modeled in `nexus-core`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::subclass::Signal;
use gtk::{gio, glib};
use nexus_core::ModelInfo;

/// Stable identifier for "which configured provider connection" a model
/// belongs to (e.g. a specific Azure OpenAI deployment, not just
/// `ProviderKind`). No such type exists yet in `nexus-core` -- the
/// automation crate's `Step::Prompt::provider_binding` is a plain `String`
/// for the same concept, so this view mirrors that rather than inventing a
/// new core type.
pub type ProviderBindingId = String;

// ---------------------------------------------------------------------
// NexusChatMessageRow
// ---------------------------------------------------------------------

mod message_row_imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/chat-message-row.ui")]
    pub struct ChatMessageRow {
        #[template_child]
        pub role_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub content_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatMessageRow {
        const NAME: &'static str = "NexusChatMessageRow";
        type Type = super::ChatMessageRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatMessageRow {}
    impl WidgetImpl for ChatMessageRow {}
    impl BoxImpl for ChatMessageRow {}
}

glib::wrapper! {
    pub struct ChatMessageRow(ObjectSubclass<message_row_imp::ChatMessageRow>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

/// Chat message role, local to the UI layer (distinct from
/// `nexus_core::Role`, which has no "error" variant since that is a
/// presentation-only concept, not something a provider ever sends).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRole {
    User,
    Assistant,
    Error,
}

impl RowRole {
    fn icon_name(self) -> &'static str {
        match self {
            RowRole::User => "avatar-default-symbolic",
            RowRole::Assistant => "emblem-system-symbolic",
            RowRole::Error => "dialog-warning-symbolic",
        }
    }
}

impl ChatMessageRow {
    pub fn new(role: RowRole, content: &str) -> Self {
        let row: Self = glib::Object::builder().build();
        let imp = row.imp();
        imp.role_icon.set_icon_name(Some(role.icon_name()));
        imp.content_label.set_text(content);
        if role == RowRole::Error {
            row.add_css_class("nexus-error-row");
        }
        row
    }

    /// Appends `delta` to the currently displayed text; used while a
    /// response is streaming in.
    pub fn append_text(&self, delta: &str) {
        let imp = self.imp();
        let mut current = imp.content_label.text().to_string();
        current.push_str(delta);
        imp.content_label.set_text(&current);
    }
}

// ---------------------------------------------------------------------
// NexusModelPickerRow
// ---------------------------------------------------------------------

mod picker_row_imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/model-picker-row.ui")]
    pub struct ModelPickerRow {
        #[template_child]
        pub checkmark: TemplateChild<gtk::Image>,
        pub provider_binding: RefCell<String>,
        pub model_id: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ModelPickerRow {
        const NAME: &'static str = "NexusModelPickerRow";
        type Type = super::ModelPickerRow;
        type ParentType = adw::ActionRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ModelPickerRow {}
    impl WidgetImpl for ModelPickerRow {}
    impl ListBoxRowImpl for ModelPickerRow {}
    impl PreferencesRowImpl for ModelPickerRow {}
    impl ActionRowImpl for ModelPickerRow {}
}

glib::wrapper! {
    pub struct ModelPickerRow(ObjectSubclass<picker_row_imp::ModelPickerRow>)
        @extends gtk::Widget, gtk::ListBoxRow, adw::PreferencesRow, adw::ActionRow,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl ModelPickerRow {
    pub fn new(provider_binding: &ProviderBindingId, model: &ModelInfo) -> Self {
        let row: Self = glib::Object::builder().build();
        row.set_title(&model.display_name);
        row.set_subtitle(provider_binding);
        *row.imp().provider_binding.borrow_mut() = provider_binding.clone();
        *row.imp().model_id.borrow_mut() = model.id.clone();
        row
    }

    pub fn provider_binding(&self) -> String {
        self.imp().provider_binding.borrow().clone()
    }

    pub fn model_id(&self) -> String {
        self.imp().model_id.borrow().clone()
    }
}

// ---------------------------------------------------------------------
// NexusChatView
// ---------------------------------------------------------------------

/// Widgets making up one model's scrollable message column.
pub(super) struct ChatColumn {
    root: gtk::Box,
    list_box: gtk::ListBox,
    current_assistant_row: RefCell<Option<ChatMessageRow>>,
    streaming: RefCell<bool>,
}

/// One chip + its backing column.
pub(super) struct ActiveModel {
    provider_binding: ProviderBindingId,
    model: ModelInfo,
    chip: gtk::FlowBoxChild,
}

fn column_id(provider_binding: &str, model_id: &str) -> String {
    format!("{provider_binding}::{model_id}")
}

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/chat-view.ui")]
    pub struct ChatView {
        #[template_child]
        pub model_chips: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub model_picker_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub model_picker_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub columns_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub composer_text: TemplateChild<gtk::TextView>,
        #[template_child]
        pub attach_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub stop_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub send_button: TemplateChild<gtk::Button>,

        pub available_models: RefCell<Vec<(ProviderBindingId, ModelInfo)>>,
        pub(super) active_models: RefCell<Vec<ActiveModel>>,
        pub(super) columns: RefCell<HashMap<String, ChatColumn>>,
        pub attached_file: RefCell<Option<gio::File>>,
    }

    impl Default for ChatView {
        fn default() -> Self {
            Self {
                model_chips: TemplateChild::default(),
                model_picker_popover: TemplateChild::default(),
                model_picker_list: TemplateChild::default(),
                columns_box: TemplateChild::default(),
                composer_text: TemplateChild::default(),
                attach_button: TemplateChild::default(),
                stop_button: TemplateChild::default(),
                send_button: TemplateChild::default(),
                available_models: RefCell::new(Vec::new()),
                active_models: RefCell::new(Vec::new()),
                columns: RefCell::new(HashMap::new()),
                attached_file: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatView {
        const NAME: &'static str = "NexusChatView";
        type Type = super::ChatView;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_signals();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("send-requested")
                        .param_types([String::static_type()])
                        .build(),
                    Signal::builder("stop-requested").build(),
                ]
            })
        }
    }

    impl WidgetImpl for ChatView {}
    impl BoxImpl for ChatView {}
}

glib::wrapper! {
    pub struct ChatView(ObjectSubclass<imp::ChatView>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for ChatView {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    /// Called by the application layer once providers are loaded (and
    /// again any time the set of configured providers/models changes).
    /// Only updates what the model-picker popover offers; does not touch
    /// any already-active model column.
    pub fn set_available_models(&self, models: Vec<(ProviderBindingId, ModelInfo)>) {
        *self.imp().available_models.borrow_mut() = models;
        self.rebuild_model_picker();
    }

    /// Appends the user's message to every active column's list as a local
    /// echo, clears the composer, and emits `send-requested` so the
    /// application layer can actually drive providers.
    pub fn send_message(&self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let imp = self.imp();
        for active in imp.active_models.borrow().iter() {
            let id = column_id(&active.provider_binding, &active.model.id);
            self.append_row(&id, RowRole::User, text);
        }
        self.emit_by_name::<()>("send-requested", &[&text.to_string()]);
    }

    /// Appends a streaming delta to `model_column_id`'s current assistant
    /// message, creating that message row the first time it is called for
    /// a given turn.
    pub fn append_delta(&self, model_column_id: &str, delta: &str) {
        let imp = self.imp();
        let columns = imp.columns.borrow();
        let Some(column) = columns.get(model_column_id) else {
            tracing::warn!("append_delta: unknown column {model_column_id}");
            return;
        };
        *column.streaming.borrow_mut() = true;
        self.update_stop_sensitivity();

        let mut current = column.current_assistant_row.borrow_mut();
        if let Some(row) = current.as_ref() {
            row.append_text(delta);
        } else {
            let row = ChatMessageRow::new(RowRole::Assistant, delta);
            column.list_box.append(&row);
            *current = Some(row);
        }
    }

    /// Marks `model_column_id` as no longer streaming, so the next
    /// `append_delta` call starts a fresh assistant message row.
    pub fn finish_stream(&self, model_column_id: &str) {
        let imp = self.imp();
        let columns = imp.columns.borrow();
        if let Some(column) = columns.get(model_column_id) {
            *column.current_assistant_row.borrow_mut() = None;
            *column.streaming.borrow_mut() = false;
        }
        drop(columns);
        self.update_stop_sensitivity();
    }

    /// Appends a visually distinct error row to `model_column_id`'s column.
    pub fn report_error(&self, model_column_id: &str, message: &str) {
        self.finish_stream(model_column_id);
        self.append_row(model_column_id, RowRole::Error, message);
    }

    pub fn connect_send_requested(
        &self,
        f: impl Fn(&Self, &str) + 'static,
    ) -> glib::SignalHandlerId {
        self.connect_local("send-requested", false, move |values| {
            let obj = values[0].get::<Self>().expect("send-requested arg 0");
            let text = values[1].get::<String>().expect("send-requested arg 1");
            f(&obj, &text);
            None
        })
    }

    pub fn connect_stop_requested(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.connect_local("stop-requested", false, move |values| {
            let obj = values[0].get::<Self>().expect("stop-requested arg 0");
            f(&obj);
            None
        })
    }

    /// Adds a new active model: a removable chip plus its own scrollable
    /// message column. No-op (returns the existing id) if this
    /// (provider_binding, model.id) pair is already active.
    pub fn add_active_model(&self, provider_binding: ProviderBindingId, model: ModelInfo) -> String {
        let id = column_id(&provider_binding, &model.id);
        let imp = self.imp();
        if imp.columns.borrow().contains_key(&id) {
            return id;
        }

        let chip_label = gtk::Label::new(Some(&model.display_name));
        let chip_remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .css_classes(["flat", "circular"])
            .build();
        let chip_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .css_classes(["nexus-model-chip"])
            .build();
        chip_box.append(&chip_label);
        chip_box.append(&chip_remove);
        imp.model_chips.insert(&chip_box, -1);
        // `FlowBox::insert` auto-wraps `chip_box` in a `GtkFlowBoxChild`;
        // walking up one parent is the documented way to get that wrapper
        // back (and `FlowBox::remove` accepts either the wrapper or the
        // original child widget, per the GTK4 docs).
        let chip = chip_box
            .parent()
            .and_downcast::<gtk::FlowBoxChild>()
            .expect("FlowBox::insert always wraps its child in a FlowBoxChild");

        chip_remove.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[strong]
            id,
            move |_| view.remove_active_model(&id)
        ));

        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["nexus-chat-column-list"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list_box)
            .build();
        let header = gtk::Label::builder()
            .label(&model.display_name)
            .css_classes(["heading"])
            .margin_top(6)
            .margin_bottom(4)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(280)
            .css_classes(["nexus-chat-column"])
            .build();
        root.append(&header);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&scrolled);
        imp.columns_box.append(&root);

        imp.active_models.borrow_mut().push(ActiveModel {
            provider_binding,
            model,
            chip,
        });
        imp.columns.borrow_mut().insert(
            id.clone(),
            ChatColumn {
                root,
                list_box,
                current_assistant_row: RefCell::new(None),
                streaming: RefCell::new(false),
            },
        );
        id
    }

    pub fn remove_active_model(&self, model_column_id: &str) {
        let imp = self.imp();
        let mut active = imp.active_models.borrow_mut();
        if let Some(pos) = active
            .iter()
            .position(|a| column_id(&a.provider_binding, &a.model.id) == model_column_id)
        {
            let removed = active.remove(pos);
            imp.model_chips.remove(&removed.chip);
        }
        drop(active);
        if let Some(column) = imp.columns.borrow_mut().remove(model_column_id) {
            imp.columns_box.remove(&column.root);
        }
    }

    fn append_row(&self, model_column_id: &str, role: RowRole, content: &str) {
        let imp = self.imp();
        let columns = imp.columns.borrow();
        if let Some(column) = columns.get(model_column_id) {
            let row = ChatMessageRow::new(role, content);
            column.list_box.append(&row);
        }
    }

    fn update_stop_sensitivity(&self) {
        let imp = self.imp();
        let any_streaming = imp
            .columns
            .borrow()
            .values()
            .any(|c| *c.streaming.borrow());
        imp.stop_button.set_sensitive(any_streaming);
    }

    fn rebuild_model_picker(&self) {
        let imp = self.imp();
        while let Some(row) = imp.model_picker_list.row_at_index(0) {
            imp.model_picker_list.remove(&row);
        }
        for (binding, model) in imp.available_models.borrow().iter() {
            let row = ModelPickerRow::new(binding, model);
            imp.model_picker_list.append(&row);
        }
    }

    fn setup_signals(&self) {
        let imp = self.imp();

        imp.model_picker_list.connect_row_activated(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, row| {
                let Some(picker_row) = row.downcast_ref::<ModelPickerRow>() else {
                    return;
                };
                let binding = picker_row.provider_binding();
                let model_id = picker_row.model_id();
                let model = view
                    .imp()
                    .available_models
                    .borrow()
                    .iter()
                    .find(|(b, m)| *b == binding && m.id == model_id)
                    .map(|(_, m)| m.clone());
                if let Some(model) = model {
                    view.add_active_model(binding, model);
                }
                view.imp().model_picker_popover.popdown();
            }
        ));

        imp.send_button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.activate_send()
        ));

        imp.stop_button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.emit_by_name::<()>("stop-requested", &[])
        ));

        imp.attach_button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.pick_attachment()
        ));

        // Ctrl+Enter / Enter-without-Shift sends, matching common chat-app
        // conventions; plain Enter alone would make multi-line composing
        // impossible with a GtkTextView.
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, keyval, _, state| {
                let is_enter =
                    keyval == gtk::gdk::Key::Return || keyval == gtk::gdk::Key::KP_Enter;
                if is_enter && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    view.activate_send();
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        imp.composer_text.add_controller(key_controller);
    }

    fn activate_send(&self) {
        let imp = self.imp();
        let buffer = imp.composer_text.buffer();
        // VERIFY: `TextBuffer::bounds()` returning `(start_iter, end_iter)`
        // is the gtk4-rs convenience method for the whole-buffer range.
        let (start, end) = buffer.bounds();
        let text = buffer.text(&start, &end, false).to_string();
        if text.trim().is_empty() {
            return;
        }
        self.send_message(&text);
        buffer.set_text("");
    }

    fn pick_attachment(&self) {
        let dialog = gtk::FileDialog::builder().title("Attach a File").build();
        let root = self.root().and_then(|r| r.downcast::<gtk::Window>().ok());
        dialog.open(
            root.as_ref(),
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |result| {
                    if let Ok(file) = result {
                        *view.imp().attached_file.borrow_mut() = Some(file);
                    }
                }
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_id_combines_binding_and_model() {
        assert_eq!(column_id("openai-default", "gpt-5"), "openai-default::gpt-5");
    }
}
