//! The automations list + editor view.
//!
//! Fully wired: listing saved automations with a trigger summary / enabled
//! switch / last-run status (`set_automations`); creating a new blank
//! automation or opening an existing one in the editor; full add / remove /
//! reorder (up/down) of steps against an in-memory
//! `Vec<nexus_automation::StepConfig>`; a six-way "Add step" chooser; and a
//! real per-step parameter editor for `Step::Prompt`, `Step::Notify`, and
//! `Step::WriteFile` (two-way bound into the `Step` enum). `Step::CliCommand`,
//! `Step::Transform`, and `Step::Condition` get full row UI (icon, summary,
//! edit/delete/reorder) but a deliberately smaller parameter editor -- see
//! `edit_simplified_step` -- since a full nested then/else/predicate editor
//! for `Condition` in particular is out of scope for this v1.
//!
//! Persistence is delegated to the application layer, mirroring
//! `chat_view`'s `send-requested` pattern: `AutomationEditor::connect_save_requested`
//! and `AutomationsView::connect_save_requested`/`connect_enabled_toggled`
//! hand back a `nexus_automation::AutomationDefinition` (serializable with
//! `serde_json::to_string`, ready for `nexus_store::automations::upsert`)
//! rather than touching the store directly.
//!
//! Note on the callback style: unlike `chat_view::ChatView`'s
//! `send-requested` (a real `glib::subclass::Signal`), the callbacks here
//! are plain stored closures (`RefCell<Vec<Box<dyn Fn(...)>>>`) rather than
//! GObject signals, because their payload (`AutomationDefinition`) is a
//! plain Rust type with no `glib::Value`/boxed-type registration -- adding
//! that machinery for a single internal callback was judged not worth the
//! complexity for this view. `connect_*` still returns nothing meaningful
//! to disconnect, which is an accepted limitation (callbacks are expected
//! to live as long as the view).

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use nexus_automation::step::{Predicate, Step, StepConfig, TransformOp};
use nexus_automation::trigger::Trigger;
use nexus_automation::AutomationDefinition;
use nexus_store::models::Automation;
use uuid::Uuid;

// ---------------------------------------------------------------------
// Pure helpers (unit-tested; no widget realization needed)
// ---------------------------------------------------------------------

pub fn step_icon_name(step: &Step) -> &'static str {
    match step {
        Step::Prompt { .. } => "chat-bubble-text-symbolic",
        Step::CliCommand { .. } => "utilities-terminal-symbolic",
        Step::Transform { .. } => "edit-find-replace-symbolic",
        Step::Condition { .. } => "test-pass-symbolic",
        Step::Notify { .. } => "preferences-system-notifications-symbolic",
        Step::WriteFile { .. } => "document-save-symbolic",
    }
}

pub fn step_summary(step: &Step) -> String {
    match step {
        Step::Prompt {
            provider_binding,
            model_id,
            output_variable,
            ..
        } => format!("Prompt {provider_binding}/{model_id} -> {output_variable}"),
        Step::CliCommand {
            agent_binding,
            output_variable,
            ..
        } => format!("Run {agent_binding} -> {output_variable}"),
        Step::Transform {
            input_variable,
            operation,
            output_variable,
        } => format!(
            "Transform {input_variable} ({}) -> {output_variable}",
            transform_op_name(operation)
        ),
        Step::Condition { input_variable, .. } => format!("If {input_variable} ..."),
        Step::Notify { title_template, .. } => format!("Notify: {title_template}"),
        Step::WriteFile { path_template, .. } => format!("Write file {path_template}"),
    }
}

fn transform_op_name(op: &TransformOp) -> &'static str {
    match op {
        TransformOp::Uppercase => "uppercase",
        TransformOp::Lowercase => "lowercase",
        TransformOp::Trim => "trim",
        TransformOp::Replace { .. } => "replace",
        TransformOp::ExtractRegex { .. } => "extract regex",
    }
}

pub fn trigger_summary(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Manual => "Manual".to_string(),
        Trigger::Scheduled { cron_expression } => format!("Scheduled: {cron_expression}"),
        Trigger::FolderChanged { path } => format!("Watches: {path}"),
    }
}

/// The six step kinds offered by the "Add step" chooser, in display order.
pub const STEP_KIND_CHOICES: &[(&str, &str)] = &[
    ("prompt", "Prompt"),
    ("cli_command", "CLI Command"),
    ("transform", "Transform"),
    ("condition", "Condition"),
    ("notify", "Notify"),
    ("write_file", "Write File"),
];

/// Builds a sensible empty default for each step kind key (as used in
/// `STEP_KIND_CHOICES`). Panics on an unknown key -- callers only ever pass
/// one of the known keys.
pub fn default_step_for_kind(kind_key: &str) -> Step {
    match kind_key {
        "prompt" => Step::Prompt {
            provider_binding: String::new(),
            model_id: String::new(),
            prompt_template: String::new(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            output_variable: "result".to_string(),
        },
        "cli_command" => Step::CliCommand {
            agent_binding: String::new(),
            prompt_template: String::new(),
            working_dir_template: "{{home}}".to_string(),
            output_variable: "cli_result".to_string(),
        },
        "transform" => Step::Transform {
            input_variable: String::new(),
            operation: TransformOp::Trim,
            output_variable: "transformed".to_string(),
        },
        "condition" => Step::Condition {
            input_variable: String::new(),
            predicate: Predicate::NonEmpty,
            then_steps: Vec::new(),
            else_steps: Vec::new(),
        },
        "notify" => Step::Notify {
            title_template: String::new(),
            body_template: String::new(),
        },
        "write_file" => Step::WriteFile {
            path_template: String::new(),
            content_variable: String::new(),
            append: false,
        },
        other => panic!("unknown step kind key: {other}"),
    }
}

fn new_step_config(kind_key: &str) -> StepConfig {
    StepConfig {
        id: Uuid::new_v4().to_string(),
        kind: default_step_for_kind(kind_key),
        on_error: None,
    }
}

// ---------------------------------------------------------------------
// NexusAutomationEditor
// ---------------------------------------------------------------------

mod editor_imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/automation-editor.ui")]
    pub struct AutomationEditor {
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub trigger_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub trigger_detail_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub steps_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub add_step_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,

        pub steps: RefCell<Vec<StepConfig>>,
        #[allow(clippy::type_complexity)]
        pub on_save: RefCell<Vec<Box<dyn Fn(&super::AutomationEditor, AutomationDefinition)>>>,
    }

    impl Default for AutomationEditor {
        fn default() -> Self {
            Self {
                name_row: TemplateChild::default(),
                trigger_row: TemplateChild::default(),
                trigger_detail_row: TemplateChild::default(),
                steps_list: TemplateChild::default(),
                add_step_button: TemplateChild::default(),
                save_button: TemplateChild::default(),
                steps: RefCell::new(Vec::new()),
                on_save: RefCell::new(Vec::new()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AutomationEditor {
        const NAME: &'static str = "NexusAutomationEditor";
        type Type = super::AutomationEditor;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AutomationEditor {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_signals();
        }
    }
    impl WidgetImpl for AutomationEditor {}
    impl BoxImpl for AutomationEditor {}
}

glib::wrapper! {
    pub struct AutomationEditor(ObjectSubclass<editor_imp::AutomationEditor>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for AutomationEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationEditor {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn clear_for_new(&self) {
        let imp = self.imp();
        imp.name_row.set_text("");
        imp.trigger_row.set_selected(0);
        imp.trigger_detail_row.set_text("");
        imp.trigger_detail_row.set_visible(false);
        *imp.steps.borrow_mut() = Vec::new();
        self.rebuild_steps_list();
    }

    pub fn load_definition(&self, definition: &AutomationDefinition) {
        let imp = self.imp();
        imp.name_row.set_text(&definition.name);
        match &definition.trigger {
            Trigger::Manual => {
                imp.trigger_row.set_selected(0);
                imp.trigger_detail_row.set_visible(false);
                imp.trigger_detail_row.set_text("");
            }
            Trigger::Scheduled { cron_expression } => {
                imp.trigger_row.set_selected(1);
                imp.trigger_detail_row.set_visible(true);
                imp.trigger_detail_row.set_text(cron_expression);
            }
            Trigger::FolderChanged { path } => {
                imp.trigger_row.set_selected(2);
                imp.trigger_detail_row.set_visible(true);
                imp.trigger_detail_row.set_text(path);
            }
        }
        *imp.steps.borrow_mut() = definition.steps.clone();
        self.rebuild_steps_list();
    }

    /// Builds an `AutomationDefinition` from the editor's current state.
    pub fn current_definition(&self) -> AutomationDefinition {
        let imp = self.imp();
        AutomationDefinition {
            name: imp.name_row.text().to_string(),
            trigger: self.current_trigger(),
            steps: imp.steps.borrow().clone(),
        }
    }

    pub fn connect_save_requested(&self, f: impl Fn(&Self, AutomationDefinition) + 'static) {
        self.imp().on_save.borrow_mut().push(Box::new(f));
    }

    fn current_trigger(&self) -> Trigger {
        let imp = self.imp();
        let detail = imp.trigger_detail_row.text().to_string();
        match imp.trigger_row.selected() {
            1 => Trigger::Scheduled { cron_expression: detail },
            2 => Trigger::FolderChanged { path: detail },
            _ => Trigger::Manual,
        }
    }

    fn setup_signals(&self) {
        let imp = self.imp();

        imp.trigger_row.connect_selected_notify(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |row| {
                editor.imp().trigger_detail_row.set_visible(row.selected() != 0);
            }
        ));

        imp.add_step_button.connect_clicked(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |_| editor.open_add_step_chooser()
        ));

        imp.save_button.connect_clicked(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |_| {
                let definition = editor.current_definition();
                let callbacks = editor.imp().on_save.borrow();
                for callback in callbacks.iter() {
                    callback(&editor, definition.clone());
                }
            }
        ));
    }

    fn rebuild_steps_list(&self) {
        let imp = self.imp();
        while let Some(row) = imp.steps_list.row_at_index(0) {
            imp.steps_list.remove(&row);
        }
        let steps = imp.steps.borrow();
        for (index, step_config) in steps.iter().enumerate() {
            let row = self.build_step_row(index, &step_config.kind, steps.len());
            imp.steps_list.append(&row);
        }
    }

    fn build_step_row(&self, index: usize, step: &Step, total: usize) -> gtk::ListBoxRow {
        let icon = gtk::Image::from_icon_name(step_icon_name(step));
        let label = gtk::Label::builder()
            .label(step_summary(step))
            .hexpand(true)
            .xalign(0.0)
            .wrap(true)
            .build();

        let up_button = gtk::Button::builder()
            .icon_name("go-up-symbolic")
            .css_classes(["flat"])
            .sensitive(index > 0)
            .build();
        let down_button = gtk::Button::builder()
            .icon_name("go-down-symbolic")
            .css_classes(["flat"])
            .sensitive(index + 1 < total)
            .build();
        let edit_button = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .css_classes(["flat"])
            .build();
        let delete_button = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .css_classes(["flat"])
            .build();

        up_button.connect_clicked(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |_| editor.move_step(index, index.wrapping_sub(1))
        ));
        down_button.connect_clicked(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |_| editor.move_step(index, index + 1)
        ));
        edit_button.connect_clicked(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |_| editor.edit_step(index)
        ));
        delete_button.connect_clicked(glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move |_| editor.delete_step(index)
        ));

        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        row_box.append(&icon);
        row_box.append(&label);
        row_box.append(&up_button);
        row_box.append(&down_button);
        row_box.append(&edit_button);
        row_box.append(&delete_button);

        gtk::ListBoxRow::builder().child(&row_box).build()
    }

    fn move_step(&self, from: usize, to: usize) {
        let imp = self.imp();
        let mut steps = imp.steps.borrow_mut();
        if to >= steps.len() {
            return;
        }
        steps.swap(from, to);
        drop(steps);
        self.rebuild_steps_list();
    }

    fn delete_step(&self, index: usize) {
        let imp = self.imp();
        imp.steps.borrow_mut().remove(index);
        self.rebuild_steps_list();
    }

    fn open_add_step_chooser(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading("Add Step")
            .body("Choose what this step should do.")
            .build();
        for (key, label) in STEP_KIND_CHOICES {
            dialog.add_response(key, label);
        }
        dialog.add_response("cancel", "Cancel");
        dialog.set_close_response("cancel");

        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = editor)]
                self,
                move |_, response| {
                    if response == "cancel" {
                        return;
                    }
                    let config = new_step_config(response);
                    editor.imp().steps.borrow_mut().push(config);
                    let new_index = editor.imp().steps.borrow().len() - 1;
                    editor.rebuild_steps_list();
                    editor.edit_step(new_index);
                }
            ),
        );

        if let Some(root) = self.root() {
            dialog.present(Some(&root));
        }
    }

    fn edit_step(&self, index: usize) {
        let step = {
            let steps = self.imp().steps.borrow();
            let Some(config) = steps.get(index) else {
                return;
            };
            config.kind.clone()
        };

        match step {
            Step::Prompt { .. } => self.edit_prompt_step(index, step),
            Step::Notify { .. } => self.edit_notify_step(index, step),
            Step::WriteFile { .. } => self.edit_write_file_step(index, step),
            Step::CliCommand { .. } | Step::Transform { .. } | Step::Condition { .. } => {
                self.edit_simplified_step(index, step)
            }
        }
    }

    fn apply_step_edit(&self, index: usize, new_step: Step) {
        let imp = self.imp();
        if let Some(config) = imp.steps.borrow_mut().get_mut(index) {
            config.kind = new_step;
        }
        self.rebuild_steps_list();
    }

    fn edit_prompt_step(&self, index: usize, step: Step) {
        let Step::Prompt {
            provider_binding,
            model_id,
            prompt_template,
            system_prompt,
            temperature,
            max_tokens,
            output_variable,
        } = step
        else {
            return;
        };

        let group = adw::PreferencesGroup::new();
        let provider_row = adw::EntryRow::builder().title("Provider binding").text(&provider_binding).build();
        let model_row = adw::EntryRow::builder().title("Model ID").text(&model_id).build();
        let prompt_row = adw::EntryRow::builder().title("Prompt template").text(&prompt_template).build();
        let system_row = adw::EntryRow::builder()
            .title("System prompt (optional)")
            .text(system_prompt.as_deref().unwrap_or(""))
            .build();
        let temperature_row = adw::EntryRow::builder()
            .title("Temperature (optional)")
            .text(temperature.map(|t| t.to_string()).unwrap_or_default())
            .build();
        let max_tokens_row = adw::EntryRow::builder()
            .title("Max tokens (optional)")
            .text(max_tokens.map(|t| t.to_string()).unwrap_or_default())
            .build();
        let output_row = adw::EntryRow::builder().title("Output variable").text(&output_variable).build();

        for row in [
            &provider_row,
            &model_row,
            &prompt_row,
            &system_row,
            &temperature_row,
            &max_tokens_row,
            &output_row,
        ] {
            group.add(row);
        }

        self.present_step_editor_dialog("Edit Prompt Step", &group, glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move || {
                let system_prompt = {
                    let t = system_row.text().to_string();
                    if t.trim().is_empty() { None } else { Some(t) }
                };
                let temperature = temperature_row.text().parse::<f32>().ok();
                let max_tokens = max_tokens_row.text().parse::<u32>().ok();
                editor.apply_step_edit(
                    index,
                    Step::Prompt {
                        provider_binding: provider_row.text().to_string(),
                        model_id: model_row.text().to_string(),
                        prompt_template: prompt_row.text().to_string(),
                        system_prompt,
                        temperature,
                        max_tokens,
                        output_variable: output_row.text().to_string(),
                    },
                );
            }
        ));
    }

    fn edit_notify_step(&self, index: usize, step: Step) {
        let Step::Notify { title_template, body_template } = step else {
            return;
        };
        let group = adw::PreferencesGroup::new();
        let title_row = adw::EntryRow::builder().title("Title template").text(&title_template).build();
        let body_row = adw::EntryRow::builder().title("Body template").text(&body_template).build();
        group.add(&title_row);
        group.add(&body_row);

        self.present_step_editor_dialog("Edit Notify Step", &group, glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move || {
                editor.apply_step_edit(
                    index,
                    Step::Notify {
                        title_template: title_row.text().to_string(),
                        body_template: body_row.text().to_string(),
                    },
                );
            }
        ));
    }

    fn edit_write_file_step(&self, index: usize, step: Step) {
        let Step::WriteFile { path_template, content_variable, append } = step else {
            return;
        };
        let group = adw::PreferencesGroup::new();
        let path_row = adw::EntryRow::builder().title("Path template").text(&path_template).build();
        let content_row = adw::EntryRow::builder()
            .title("Content variable")
            .text(&content_variable)
            .build();
        let append_row = adw::SwitchRow::builder().title("Append instead of overwrite").active(append).build();
        group.add(&path_row);
        group.add(&content_row);
        group.add(&append_row);

        self.present_step_editor_dialog("Edit Write File Step", &group, glib::clone!(
            #[weak(rename_to = editor)]
            self,
            move || {
                editor.apply_step_edit(
                    index,
                    Step::WriteFile {
                        path_template: path_row.text().to_string(),
                        content_variable: content_row.text().to_string(),
                        append: append_row.is_active(),
                    },
                );
            }
        ));
    }

    /// Simplified editor for `CliCommand`, `Transform`, and `Condition`:
    /// only their most essential field(s) are editable here. A full
    /// editor (in particular for `Condition`'s nested `then_steps`/
    /// `else_steps`) is out of scope for this v1 -- see the module doc.
    fn edit_simplified_step(&self, index: usize, step: Step) {
        let group = adw::PreferencesGroup::new();
        match step {
            Step::CliCommand {
                agent_binding,
                prompt_template,
                working_dir_template,
                output_variable,
            } => {
                let agent_row = adw::EntryRow::builder().title("Agent binding").text(&agent_binding).build();
                let prompt_row = adw::EntryRow::builder().title("Prompt template").text(&prompt_template).build();
                group.add(&agent_row);
                group.add(&prompt_row);
                self.present_step_editor_dialog("Edit CLI Command Step", &group, glib::clone!(
                    #[weak(rename_to = editor)]
                    self,
                    move || {
                        editor.apply_step_edit(
                            index,
                            Step::CliCommand {
                                agent_binding: agent_row.text().to_string(),
                                prompt_template: prompt_row.text().to_string(),
                                working_dir_template: working_dir_template.clone(),
                                output_variable: output_variable.clone(),
                            },
                        );
                    }
                ));
            }
            Step::Transform {
                input_variable,
                operation,
                output_variable,
            } => {
                let input_row = adw::EntryRow::builder().title("Input variable").text(&input_variable).build();
                let output_row = adw::EntryRow::builder().title("Output variable").text(&output_variable).build();
                group.add(&input_row);
                group.add(&output_row);
                self.present_step_editor_dialog("Edit Transform Step", &group, glib::clone!(
                    #[weak(rename_to = editor)]
                    self,
                    move || {
                        editor.apply_step_edit(
                            index,
                            Step::Transform {
                                input_variable: input_row.text().to_string(),
                                operation: operation.clone(),
                                output_variable: output_row.text().to_string(),
                            },
                        );
                    }
                ));
            }
            Step::Condition { input_variable, predicate, then_steps, else_steps } => {
                let input_row = adw::EntryRow::builder().title("Input variable").text(&input_variable).build();
                group.add(&input_row);
                self.present_step_editor_dialog("Edit Condition Step", &group, glib::clone!(
                    #[weak(rename_to = editor)]
                    self,
                    move || {
                        editor.apply_step_edit(
                            index,
                            Step::Condition {
                                input_variable: input_row.text().to_string(),
                                predicate: predicate.clone(),
                                then_steps: then_steps.clone(),
                                else_steps: else_steps.clone(),
                            },
                        );
                    }
                ));
            }
            _ => {}
        }
    }

    fn present_step_editor_dialog(
        &self,
        heading: &str,
        group: &adw::PreferencesGroup,
        on_save: impl Fn() + 'static,
    ) {
        let dialog = adw::AlertDialog::builder().heading(heading).extra_child(group).build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_close_response("cancel");
        dialog.connect_response(None, move |_, response| {
            if response == "save" {
                on_save();
            }
        });
        if let Some(root) = self.root() {
            dialog.present(Some(&root));
        }
    }
}

// ---------------------------------------------------------------------
// NexusAutomationsView
// ---------------------------------------------------------------------

/// What `AutomationsView::set_automations` needs beyond the raw store row:
/// a human-readable trigger summary (parsed from `definition_json` by the
/// application layer, since this view never parses JSON on its own for
/// anything other than the editor it owns) and the most recent run's
/// status, if any.
#[derive(Debug, Clone)]
pub struct AutomationSummary {
    pub automation: Automation,
    pub trigger_summary: String,
    pub last_run_status: Option<String>,
}

mod view_imp {
    use super::*;
    use std::cell::OnceCell;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/automations-view.ui")]
    pub struct AutomationsView {
        #[template_child]
        pub automation_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub new_automation_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub editor_container: TemplateChild<gtk::Box>,

        pub editor: OnceCell<AutomationEditor>,
        pub automations: RefCell<Vec<AutomationSummary>>,
        pub current_id: RefCell<Option<String>>,
        #[allow(clippy::type_complexity)]
        pub on_save: RefCell<Vec<Box<dyn Fn(&super::AutomationsView, Option<String>, AutomationDefinition)>>>,
        #[allow(clippy::type_complexity)]
        pub on_enabled_toggled: RefCell<Vec<Box<dyn Fn(&super::AutomationsView, String, bool)>>>,
    }

    impl Default for AutomationsView {
        fn default() -> Self {
            Self {
                automation_list: TemplateChild::default(),
                new_automation_button: TemplateChild::default(),
                editor_container: TemplateChild::default(),
                editor: OnceCell::new(),
                automations: RefCell::new(Vec::new()),
                current_id: RefCell::new(None),
                on_save: RefCell::new(Vec::new()),
                on_enabled_toggled: RefCell::new(Vec::new()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AutomationsView {
        const NAME: &'static str = "NexusAutomationsView";
        type Type = super::AutomationsView;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AutomationsView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_empty_editor();
            self.obj().setup_signals();
        }
    }
    impl WidgetImpl for AutomationsView {}
    impl BoxImpl for AutomationsView {}
}

glib::wrapper! {
    pub struct AutomationsView(ObjectSubclass<view_imp::AutomationsView>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for AutomationsView {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationsView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_automations(&self, automations: Vec<AutomationSummary>) {
        let imp = self.imp();
        while let Some(row) = imp.automation_list.row_at_index(0) {
            imp.automation_list.remove(&row);
        }
        for summary in &automations {
            let row = self.build_automation_row(summary);
            imp.automation_list.append(&row);
        }
        *imp.automations.borrow_mut() = automations;
    }

    pub fn connect_save_requested(
        &self,
        f: impl Fn(&Self, Option<String>, AutomationDefinition) + 'static,
    ) {
        self.imp().on_save.borrow_mut().push(Box::new(f));
    }

    pub fn connect_enabled_toggled(&self, f: impl Fn(&Self, String, bool) + 'static) {
        self.imp().on_enabled_toggled.borrow_mut().push(Box::new(f));
    }

    fn build_automation_row(&self, summary: &AutomationSummary) -> adw::ActionRow {
        let subtitle = match &summary.last_run_status {
            Some(status) => format!("{} - last run: {status}", summary.trigger_summary),
            None => format!("{} - never run", summary.trigger_summary),
        };
        let row = adw::ActionRow::builder()
            .title(&summary.automation.name)
            .subtitle(subtitle)
            .activatable(true)
            .build();

        let enabled_switch = gtk::Switch::builder()
            .active(summary.automation.enabled)
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&enabled_switch);

        // VERIFY: `Switch::connect_state_set`'s closure return type
        // (`glib::Propagation` here) against gtk4-rs 0.11.x -- some
        // binding generations used a plain `bool` for this signal instead.
        let id = summary.automation.id.clone();
        enabled_switch.connect_state_set(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[strong]
            id,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, state| {
                let callbacks = view.imp().on_enabled_toggled.borrow();
                for callback in callbacks.iter() {
                    callback(&view, id.clone(), state);
                }
                glib::Propagation::Proceed
            }
        ));

        row.connect_activated(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[strong]
            id,
            move |_| view.open_automation(&id)
        ));

        row
    }

    fn open_automation(&self, id: &str) {
        let imp = self.imp();
        let Some(summary) = imp
            .automations
            .borrow()
            .iter()
            .find(|s| s.automation.id == id)
            .cloned()
        else {
            return;
        };
        let Ok(definition) =
            serde_json::from_str::<AutomationDefinition>(&summary.automation.definition_json)
        else {
            tracing::warn!("automation {id} has malformed definition_json");
            return;
        };
        *imp.current_id.borrow_mut() = Some(id.to_string());
        self.ensure_editor().load_definition(&definition);
        self.show_editor();
    }

    fn setup_empty_editor(&self) {
        let empty = super::new_empty_state(
            "system-run-symbolic",
            "No Automations Yet",
            "Create an automation to chain prompts, CLI commands, and notifications together.",
        );
        self.imp().editor_container.append(&empty);
    }

    fn ensure_editor(&self) -> AutomationEditor {
        let imp = self.imp();
        if let Some(editor) = imp.editor.get() {
            return editor.clone();
        }
        let editor = AutomationEditor::new();
        editor.connect_save_requested(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_, definition| {
                let id = view.imp().current_id.borrow().clone();
                let callbacks = view.imp().on_save.borrow();
                for callback in callbacks.iter() {
                    callback(&view, id.clone(), definition.clone());
                }
            }
        ));
        let _ = imp.editor.set(editor.clone());
        editor
    }

    fn show_editor(&self) {
        let imp = self.imp();
        let editor = self.ensure_editor();
        while let Some(child) = imp.editor_container.first_child() {
            imp.editor_container.remove(&child);
        }
        imp.editor_container.append(&editor);
    }

    fn setup_signals(&self) {
        self.imp().new_automation_button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| {
                *view.imp().current_id.borrow_mut() = None;
                let editor = view.ensure_editor();
                editor.clear_for_new();
                view.show_editor();
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_summary_formats_each_variant() {
        assert_eq!(trigger_summary(&Trigger::Manual), "Manual");
        assert_eq!(
            trigger_summary(&Trigger::Scheduled {
                cron_expression: "0 0 * * * *".to_string()
            }),
            "Scheduled: 0 0 * * * *"
        );
        assert_eq!(
            trigger_summary(&Trigger::FolderChanged {
                path: "/tmp/inbox".to_string()
            }),
            "Watches: /tmp/inbox"
        );
    }

    #[test]
    fn step_summary_mentions_output_variable() {
        let step = Step::Prompt {
            provider_binding: "openai".to_string(),
            model_id: "gpt-5".to_string(),
            prompt_template: "hi".to_string(),
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            output_variable: "reply".to_string(),
        };
        assert!(step_summary(&step).contains("reply"));
    }

    #[test]
    fn default_step_for_kind_covers_all_six_choices() {
        for (key, _) in STEP_KIND_CHOICES {
            // Must not panic for any advertised choice.
            let _ = default_step_for_kind(key);
        }
    }

    #[test]
    #[should_panic]
    fn default_step_for_kind_panics_on_unknown_key() {
        let _ = default_step_for_kind("not-a-real-kind");
    }
}
