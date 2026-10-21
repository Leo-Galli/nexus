//! The CLI coding agents view: a sidebar of detected/known agents plus a
//! content area that hosts either an interactive `vte4::Terminal` session
//! or a structured JSONL output view.
//!
//! Fully wired:
//! - `set_agents` populates the sidebar from whatever
//!   `nexus_cli_agents::detect::detect_all` result the application layer
//!   hands in.
//! - "New Session": builds the real spawn argv (honoring
//!   `enabled-host-cli-execution` from GSettings, wrapping in
//!   `flatpak-spawn --host ...` when sandboxed, exactly mirroring
//!   `nexus_cli_agents::runner::FlatpakHostCommandRunner`), shows a real
//!   `AdwAlertDialog` confirmation listing the exact command line, and on
//!   confirmation spawns a real `vte4::Terminal` as a new session-stack
//!   page. This path is fully self-contained: it needs no `Provider`/
//!   `CliAgent` trait object, only the agent's binary name, so it works
//!   end-to-end without the application layer's help.
//! - `display_json_output`: renders parsed JSONL lines (or raw text for
//!   lines that are not valid JSON) in a `GtkListBox`, fully implemented
//!   and independently testable (`parse_jsonl_line`).
//! - `build_spawn_argv` and `command_preview_text` are pure, unit-tested
//!   helper functions.
//!
//! Stubbed / left for the application layer:
//! - "Run Once" (the non-interactive JSON-mode path) cannot be fully
//!   implemented here: building the real invocation requires calling
//!   `CliAgent::build_noninteractive_command`, which needs the actual
//!   configured `Arc<dyn CliAgent>` trait object this view never holds
//!   (only the `CliAgentInfo`/`CliDetectionResult` pair passed into
//!   `set_agents`). Clicking it emits the `run-once-requested` signal
//!   (agent id); the application layer is expected to: build the
//!   invocation, show its own confirmation dialog (reusing the exported
//!   `command_preview_text` helper for consistent wording), run it via a
//!   real `CommandRunner` on the Tokio runtime, and call back
//!   `display_json_output` with the result.
//! // TODO: wire to real CliAgent/CommandRunner instances via the
//! // application layer's registry, same pattern as chat_view's
//! // send-requested signal.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::subclass::Signal;
use gtk::{gio, glib};
use nexus_core::{CliAgentInfo, CliDetectionResult, LoginStatus};
use vte::TerminalExtManual;

use crate::config;

// ---------------------------------------------------------------------
// Pure helpers (unit-tested below; no widget realization needed)
// ---------------------------------------------------------------------

/// Builds the actual program + args that will be spawned for `program`
/// with `args` in `cwd`, honoring whether Nexus must go through the host
/// via `flatpak-spawn --host` (`sandboxed == true`, i.e. the user has
/// enabled `enabled-host-cli-execution` and Nexus is running sandboxed).
/// Mirrors `nexus_cli_agents::runner::FlatpakHostCommandRunner::run`
/// exactly, so the confirmation dialog shown before spawning reflects the
/// real command.
pub fn build_spawn_argv(
    program: &str,
    args: &[String],
    cwd: &Path,
    sandboxed: bool,
) -> (String, Vec<String>) {
    if sandboxed {
        let mut full_args = vec!["--host".to_string(), format!("--directory={}", cwd.display())];
        full_args.push(program.to_string());
        full_args.extend(args.iter().cloned());
        ("flatpak-spawn".to_string(), full_args)
    } else {
        (program.to_string(), args.to_vec())
    }
}

/// Human-readable command-line preview shown in the confirmation dialog.
pub fn command_preview_text(program: &str, args: &[String], cwd: &Path) -> String {
    let mut line = program.to_string();
    for arg in args {
        line.push(' ');
        line.push_str(arg);
    }
    format!("{line}\n\nWorking directory: {}", cwd.display())
}

/// One line of a CLI agent's structured JSON output, parsed (best-effort)
/// for display. Lines that are not valid JSON are shown verbatim.
pub fn parse_jsonl_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => serde_json::to_string_pretty(&value).unwrap_or_else(|_| trimmed.to_string()),
        Err(_) => trimmed.to_string(),
    }
}

// ---------------------------------------------------------------------
// NexusAgentSessionRow
// ---------------------------------------------------------------------

mod row_imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/agent-session-row.ui")]
    pub struct AgentSessionRow {
        #[template_child]
        pub agent_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub installed_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub version_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub login_badge: TemplateChild<gtk::Label>,
        #[template_child]
        pub run_once_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub new_session_button: TemplateChild<gtk::Button>,
        pub agent_id: RefCell<String>,
        pub binary_path: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AgentSessionRow {
        const NAME: &'static str = "NexusAgentSessionRow";
        type Type = super::AgentSessionRow;
        type ParentType = adw::ActionRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AgentSessionRow {}
    impl WidgetImpl for AgentSessionRow {}
    impl ListBoxRowImpl for AgentSessionRow {}
    impl PreferencesRowImpl for AgentSessionRow {}
    impl ActionRowImpl for AgentSessionRow {}
}

glib::wrapper! {
    pub struct AgentSessionRow(ObjectSubclass<row_imp::AgentSessionRow>)
        @extends gtk::Widget, gtk::ListBoxRow, adw::PreferencesRow, adw::ActionRow,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl AgentSessionRow {
    pub fn new(info: &CliAgentInfo, detection: &CliDetectionResult) -> Self {
        let row: Self = glib::Object::builder().build();
        row.set_title(info.display_name);
        *row.imp().agent_id.borrow_mut() = info.id.to_string();
        *row.imp().binary_path.borrow_mut() = detection.binary_path.clone();

        let imp = row.imp();
        imp.installed_badge.set_text(if detection.installed {
            "Installed"
        } else {
            "Not found"
        });
        imp.version_badge
            .set_text(detection.version.as_deref().unwrap_or(""));
        imp.version_badge
            .set_visible(detection.version.is_some());
        imp.login_badge.set_text(match detection.login_status {
            LoginStatus::LoggedIn => "Logged in",
            LoginStatus::LoggedOut => "Logged out",
            LoginStatus::Unknown => "",
        });
        imp.login_badge
            .set_visible(detection.login_status != LoginStatus::Unknown);

        row.set_sensitive(detection.installed);
        row
    }

    pub fn agent_id(&self) -> String {
        self.imp().agent_id.borrow().clone()
    }

    pub fn binary_path(&self) -> Option<String> {
        self.imp().binary_path.borrow().clone()
    }
}

// ---------------------------------------------------------------------
// NexusAgentsView
// ---------------------------------------------------------------------

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/agents-view.ui")]
    pub struct AgentsView {
        #[template_child]
        pub agent_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub confirm_before_actions_check: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub session_stack: TemplateChild<gtk::Stack>,
        pub settings: gio::Settings,
        pub agents: RefCell<Vec<(CliAgentInfo, CliDetectionResult)>>,
    }

    impl Default for AgentsView {
        fn default() -> Self {
            Self {
                agent_list: TemplateChild::default(),
                confirm_before_actions_check: TemplateChild::default(),
                session_stack: TemplateChild::default(),
                settings: gio::Settings::new(config::APP_ID),
                agents: RefCell::new(Vec::new()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AgentsView {
        const NAME: &'static str = "NexusAgentsView";
        type Type = super::AgentsView;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AgentsView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_empty_page();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![Signal::builder("run-once-requested")
                    .param_types([String::static_type()])
                    .build()]
            })
        }
    }

    impl WidgetImpl for AgentsView {}
    impl BoxImpl for AgentsView {}
}

glib::wrapper! {
    pub struct AgentsView(ObjectSubclass<imp::AgentsView>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for AgentsView {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentsView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_agents(&self, agents: Vec<(CliAgentInfo, CliDetectionResult)>) {
        let imp = self.imp();
        while let Some(row) = imp.agent_list.row_at_index(0) {
            imp.agent_list.remove(&row);
        }
        for (info, detection) in &agents {
            let row = AgentSessionRow::new(info, detection);
            let row_imp = row.imp();

            row_imp.new_session_button.connect_clicked(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                row,
                move |_| view.confirm_and_start_session(&row)
            ));
            row_imp.run_once_button.connect_clicked(glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                row,
                move |_| view.emit_by_name::<()>("run-once-requested", &[&row.agent_id()])
            ));

            imp.agent_list.append(&row);
        }
        *imp.agents.borrow_mut() = agents;
    }

    /// Whether the "Confirm before running file-touching commands" checkbox
    /// is active. The application layer should consult this before running
    /// a "Run Once" (non-interactive) command it builds itself; the
    /// interactive "New Session" path above always confirms regardless of
    /// this setting.
    pub fn confirm_before_actions(&self) -> bool {
        self.imp().confirm_before_actions_check.is_active()
    }

    pub fn connect_run_once_requested(
        &self,
        f: impl Fn(&Self, &str) + 'static,
    ) -> glib::SignalHandlerId {
        self.connect_local("run-once-requested", false, move |values| {
            let obj = values[0].get::<Self>().expect("run-once-requested arg 0");
            let agent_id = values[1]
                .get::<String>()
                .expect("run-once-requested arg 1");
            f(&obj, &agent_id);
            None
        })
    }

    /// Renders `lines` (raw JSONL, one entry per output line) as a
    /// `GtkListBox`, replacing whatever the given session page currently
    /// shows. `page_name` should already exist (created by
    /// `confirm_and_start_session`'s terminal path uses a different page
    /// kind; this is for the non-interactive "Run Once" JSON path the
    /// application layer drives).
    pub fn display_json_output(&self, agent_id: &str, lines: &[String]) {
        let page_name = format!("json:{agent_id}");
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(12)
            .build();
        for line in lines {
            let rendered = parse_jsonl_line(line);
            if rendered.is_empty() {
                continue;
            }
            let label = gtk::Label::builder()
                .label(&rendered)
                .wrap(true)
                .xalign(0.0)
                .selectable(true)
                .build();
            list.append(&label);
        }
        let scrolled = gtk::ScrolledWindow::builder().child(&list).build();

        let imp = self.imp();
        if let Some(existing) = imp.session_stack.child_by_name(&page_name) {
            imp.session_stack.remove(&existing);
        }
        imp.session_stack.add_named(&scrolled, Some(&page_name));
        imp.session_stack.set_visible_child_name(&page_name);
    }

    fn setup_empty_page(&self) {
        let empty = super::new_empty_state(
            "utilities-terminal-symbolic",
            "No Agent Session Open",
            "Choose a CLI agent on the left and start a new session.",
        );
        self.imp()
            .session_stack
            .add_named(&empty, Some("empty"));
        self.imp().session_stack.set_visible_child_name("empty");
    }

    fn confirm_and_start_session(&self, row: &AgentSessionRow) {
        let Some(program) = row.binary_path() else {
            return;
        };
        let args: Vec<String> = Vec::new();
        let cwd = glib::home_dir();
        let sandboxed = self.imp().settings.boolean("enabled-host-cli-execution")
            && std::path::Path::new("/.flatpak-info").exists();
        let (spawn_program, spawn_args) = build_spawn_argv(&program, &args, &cwd, sandboxed);
        let preview = command_preview_text(&spawn_program, &spawn_args, &cwd);

        let dialog = adw::AlertDialog::builder()
            .heading("Run this command?")
            .body(preview)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("run", "Run");
        dialog.set_response_appearance("run", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let agent_id = row.agent_id();
        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[strong]
                spawn_program,
                #[strong]
                spawn_args,
                #[strong]
                cwd,
                #[strong]
                agent_id,
                move |_, response| {
                    if response == "run" {
                        view.spawn_terminal_session(&agent_id, &spawn_program, &spawn_args, &cwd);
                    }
                }
            ),
        );

        if let Some(root) = self.root() {
            dialog.present(Some(&root));
        }
    }

    fn spawn_terminal_session(
        &self,
        agent_id: &str,
        program: &str,
        args: &[String],
        cwd: &PathBuf,
    ) {
        let terminal = vte::Terminal::new();
        let page_name = format!("terminal:{agent_id}");

        let imp = self.imp();
        if let Some(existing) = imp.session_stack.child_by_name(&page_name) {
            imp.session_stack.remove(&existing);
        }
        imp.session_stack.add_named(&terminal, Some(&page_name));
        imp.session_stack.set_visible_child_name(&page_name);

        let mut argv: Vec<&str> = vec![program];
        argv.extend(args.iter().map(String::as_str));
        let cwd_str = cwd.to_string_lossy().to_string();

        // VERIFY: `vte4::Terminal::spawn_async`'s exact parameter list
        // (argument order/types of `child_setup` and `cancellable` in
        // particular) against vte4 0.10's generated bindings -- this
        // matches the shape documented by the underlying `vte_terminal_
        // spawn_async` C API as of VTE 0.70+.
        terminal.spawn_async(
            vte::PtyFlags::DEFAULT,
            Some(&cwd_str),
            &argv,
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gio::Cancellable::NONE,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |result| {
                    if let Err(err) = result {
                        tracing::warn!("failed to spawn agent session: {err}");
                        view.show_toast_if_possible(&format!(
                            "Failed to start session: {err}"
                        ));
                    }
                }
            ),
        );
    }

    fn show_toast_if_possible(&self, text: &str) {
        if let Some(window) = self
            .root()
            .and_then(|r| r.downcast::<crate::window::NexusWindow>().ok())
        {
            window.show_toast(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_spawn_argv_direct_when_not_sandboxed() {
        let (program, args) = build_spawn_argv(
            "claude",
            &["--version".to_string()],
            Path::new("/home/user"),
            false,
        );
        assert_eq!(program, "claude");
        assert_eq!(args, vec!["--version".to_string()]);
    }

    #[test]
    fn build_spawn_argv_wraps_with_flatpak_spawn_when_sandboxed() {
        let (program, args) =
            build_spawn_argv("claude", &["--version".to_string()], Path::new("/home/user"), true);
        assert_eq!(program, "flatpak-spawn");
        assert_eq!(
            args,
            vec![
                "--host".to_string(),
                "--directory=/home/user".to_string(),
                "claude".to_string(),
                "--version".to_string(),
            ]
        );
    }

    #[test]
    fn command_preview_includes_program_args_and_cwd() {
        let text = command_preview_text(
            "flatpak-spawn",
            &["--host".to_string(), "claude".to_string()],
            Path::new("/home/user"),
        );
        assert!(text.contains("flatpak-spawn --host claude"));
        assert!(text.contains("/home/user"));
    }

    #[test]
    fn parse_jsonl_line_pretty_prints_valid_json() {
        let rendered = parse_jsonl_line(r#"{"type":"text","value":"hi"}"#);
        assert!(rendered.contains("\"type\""));
        assert!(rendered.contains("hi"));
    }

    #[test]
    fn parse_jsonl_line_passes_through_non_json() {
        assert_eq!(parse_jsonl_line("plain log line"), "plain log line");
    }

    #[test]
    fn parse_jsonl_line_empty_is_empty() {
        assert_eq!(parse_jsonl_line("   "), "");
    }
}
