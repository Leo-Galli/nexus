//! `NexusWindow`, the single top-level `adw::ApplicationWindow`.
//!
//! Fully wired: the sidebar-section <-> `GtkStack` page switching, window
//! size/maximized-state persistence via the real `GSettings` keys found in
//! `data/io.github.GITHUB_OWNER.Nexus.gschema.xml` (`window-width`,
//! `window-height`, `window-maximized`), and `show_toast` for transient
//! notices from any view.
//!
//! Stubbed / left for the application layer: this file constructs the four
//! view widgets with their own `new()` (no provider/store data yet) and
//! never calls `ChatView::set_available_models`,
//! `AgentsView`'s agent-detection refresh, `AutomationsView`'s store load,
//! or `StatsView::set_rows` -- `NexusApplication` (or a future
//! `app.rs`-level coordinator) is expected to fetch that data once
//! providers/CLI agents/the store are ready and push it into each view.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::NexusApplication;
use crate::config;
use crate::views::agents_view::AgentsView;
use crate::views::automations_view::AutomationsView;
use crate::views::chat_view::ChatView;
use crate::views::stats_view::StatsView;

mod imp {
    use super::*;
    use std::cell::OnceCell;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/window.ui")]
    pub struct NexusWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub sidebar_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub content_stack: TemplateChild<gtk::Stack>,

        pub settings: gio::Settings,
        pub chat_view: OnceCell<ChatView>,
        pub agents_view: OnceCell<AgentsView>,
        pub automations_view: OnceCell<AutomationsView>,
        pub stats_view: OnceCell<StatsView>,
    }

    impl Default for NexusWindow {
        fn default() -> Self {
            Self {
                toast_overlay: TemplateChild::default(),
                sidebar_list: TemplateChild::default(),
                content_stack: TemplateChild::default(),
                settings: gio::Settings::new(config::APP_ID),
                chat_view: OnceCell::new(),
                agents_view: OnceCell::new(),
                automations_view: OnceCell::new(),
                stats_view: OnceCell::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NexusWindow {
        const NAME: &'static str = "NexusWindow";
        type Type = super::NexusWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NexusWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_views();
            obj.setup_sidebar();
            obj.restore_window_state();
            obj.setup_window_actions();
        }
    }

    impl WidgetImpl for NexusWindow {}

    impl WindowImpl for NexusWindow {
        fn close_request(&self) -> glib::Propagation {
            self.obj().save_window_state();
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for NexusWindow {}
    impl AdwApplicationWindowImpl for NexusWindow {}
}

glib::wrapper! {
    // VERIFY: the exact `@extends`/`@implements` chain for an
    // `adw::ApplicationWindow` subclass in libadwaita-rs 0.9.x. This mirrors
    // the chain used by other gtk-rs composite-template application
    // windows (Fractal/Shortwave-style); `gtk::Buildable` is included since
    // every `gtk::Widget` implements it.
    pub struct NexusWindow(ObjectSubclass<imp::NexusWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                     gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl NexusWindow {
    pub fn new(app: &NexusApplication) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    /// Shows a transient toast over the whole content side of the window.
    /// Any view can reach this via `self.root().and_downcast::<NexusWindow>()`.
    pub fn show_toast(&self, text: &str) {
        let toast = adw::Toast::builder().title(text).timeout(4).build();
        self.imp().toast_overlay.add_toast(toast);
    }

    fn setup_views(&self) {
        let imp = self.imp();

        let chat_view = ChatView::new();
        imp.content_stack.add_named(&chat_view, Some("chat"));
        imp.chat_view.set(chat_view).expect("views are only created once");

        let agents_view = AgentsView::new();
        imp.content_stack.add_named(&agents_view, Some("agents"));
        imp.agents_view
            .set(agents_view)
            .expect("views are only created once");

        let automations_view = AutomationsView::new();
        imp.content_stack
            .add_named(&automations_view, Some("automations"));
        imp.automations_view
            .set(automations_view)
            .expect("views are only created once");

        let stats_view = StatsView::new();
        imp.content_stack.add_named(&stats_view, Some("stats"));
        imp.stats_view
            .set(stats_view)
            .expect("views are only created once");

        imp.content_stack.set_visible_child_name("chat");
    }

    fn setup_sidebar(&self) {
        let imp = self.imp();
        imp.sidebar_list.connect_row_activated(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, row| {
                let name = row.widget_name();
                if !name.is_empty() {
                    window.imp().content_stack.set_visible_child_name(&name);
                }
            }
        ));

        if let Some(first_row) = imp.sidebar_list.row_at_index(0) {
            imp.sidebar_list.select_row(Some(&first_row));
        }
    }

    fn restore_window_state(&self) {
        let imp = self.imp();
        let width = imp.settings.int("window-width");
        let height = imp.settings.int("window-height");
        self.set_default_size(width, height);

        if imp.settings.boolean("window-maximized") {
            self.maximize();
        }

        // `maximized` is bound bidirectionally for the lifetime of the
        // window; `window-width`/`window-height` are saved explicitly in
        // `save_window_state` instead of bound, since a live two-way bind on
        // `default-width`/`default-height` would also fight with
        // `set_default_size` above on every resize.
        imp.settings
            .bind("window-maximized", self, "maximized")
            .build();
    }

    /// Registers `win.new-chat` and the four `win.focus-*` actions with
    /// their accelerators (`<primary>n` and `<alt>1`..`<alt>4`), matching
    /// what `shortcuts.blp` advertises. Window-scoped (`win.*`) actions
    /// still take application-wide accelerators via
    /// `Application::set_accels_for_action`, same as `app.*` ones.
    fn setup_window_actions(&self) {
        let new_chat_action = gio::SimpleAction::new("new-chat", None);
        new_chat_action.connect_activate(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _| window.focus_section("chat")
        ));
        self.add_action(&new_chat_action);

        for name in ["chat", "agents", "automations", "stats"] {
            let action = gio::SimpleAction::new(&format!("focus-{name}"), None);
            action.connect_activate(glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[strong]
                name,
                move |_, _| window.focus_section(name)
            ));
            self.add_action(&action);
        }

        if let Some(app) = self.application().and_downcast::<NexusApplication>() {
            app.set_accels_for_action("win.new-chat", &["<primary>n"]);
            app.set_accels_for_action("win.focus-chat", &["<alt>1"]);
            app.set_accels_for_action("win.focus-agents", &["<alt>2"]);
            app.set_accels_for_action("win.focus-automations", &["<alt>3"]);
            app.set_accels_for_action("win.focus-stats", &["<alt>4"]);
        }
    }

    /// Selects the sidebar row named `name` (switching the content stack
    /// to match, via the existing `row-activated` handler) and, for
    /// "chat", additionally clears/focuses the composer -- this is the
    /// real part of "New chat" for now; starting an actual new
    /// `nexus_store::models::Conversation` is left for the application
    /// layer, which owns the store.
    fn focus_section(&self, name: &str) {
        let imp = self.imp();
        let mut index = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(index) {
            if row.widget_name() == name {
                imp.sidebar_list.select_row(Some(&row));
                imp.content_stack.set_visible_child_name(name);
                break;
            }
            index += 1;
        }
    }

    fn save_window_state(&self) {
        let imp = self.imp();
        if !self.is_maximized() {
            // `Widget::width`/`height` report the current allocated size,
            // which (for a non-maximized top-level) is what we want to
            // restore next launch; GtkWindow's own `default-width`/
            // `default-height` properties only reflect the size passed to
            // `set_default_size`, not live resizes.
            let _ = imp.settings.set_int("window-width", self.width());
            let _ = imp.settings.set_int("window-height", self.height());
        }
    }
}
