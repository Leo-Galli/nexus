//! The local usage statistics dashboard.
//!
//! Fully wired:
//! - `set_rows` computes real aggregation-of-the-aggregates math (see
//!   `summarize`, unit-tested) and updates five stat cards plus a simple
//!   per-bucket bar chart.
//! - The chart is a row of plain `GtkBox` "bars" sized via
//!   `set_size_request` proportional to each bucket's token total, per the
//!   module-level note in `stats-view.blp`: `cairo-rs`/`gtk::cairo` are not
//!   in `Cargo.toml`, so no `GtkDrawingArea` draw-func is used.
//! - The granularity and date-range switchers are plain linked
//!   `GtkToggleButton`s (see `stats-view.blp` for why, not `AdwToggleGroup`)
//!   with mutual exclusivity enforced in Rust.
//! - `connect_export_requested` plus a fully-implemented `gtk::FileDialog`
//!   save flow that writes `nexus_stats::csv_export::rows_to_csv` of
//!   whatever rows `set_rows` was last called with -- this needs no
//!   provider/store access and works standalone.
//!
//! Stubbed / left for the application layer:
//! - `set_provider_options`/`set_model_options` only populate the filter
//!   dropdowns; `connect_filters_changed` fires whenever any filter control
//!   changes (granularity, date preset, provider, model) so the
//!   application layer can re-query `nexus_stats::aggregate::aggregate`
//!   and call `set_rows` again with fresh data. This view never queries
//!   the store itself.

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use chrono::{DateTime, Duration, Utc};
use glib::subclass::Signal;
use gtk::{gio, glib};
use nexus_stats::aggregate::{AggregatedRow, BucketGranularity};

// ---------------------------------------------------------------------
// Pure helpers (unit-tested; no widget realization needed)
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePreset {
    Last7Days,
    Last30Days,
    AllTime,
}

/// `(since, until)` filter bounds for a preset, anchored at `now`.
/// `until` is always `None` (meaning "through now") since every preset
/// means "from N days ago through now", never a closed historical range.
pub fn date_preset_range(preset: DatePreset, now: DateTime<Utc>) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    match preset {
        DatePreset::Last7Days => (Some(now - Duration::days(7)), None),
        DatePreset::Last30Days => (Some(now - Duration::days(30)), None),
        DatePreset::AllTime => (None, None),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StatsSummary {
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// `None` only when there are no rows at all; otherwise the sum over
    /// every row with a known price, which may be less than the "true"
    /// total if some rows had an unknown price -- see `any_cost_unknown`.
    pub total_cost_usd: Option<f64>,
    pub any_cost_unknown: bool,
    pub avg_latency_ms: f64,
    pub total_errors: u64,
    pub error_rate: f64,
}

/// Aggregation-of-the-aggregates: sums `request_count`/tokens/errors over
/// every row, sums `estimated_cost_usd` only over rows where it is
/// `Some` (flagging `any_cost_unknown` if any row's price was unknown, so
/// callers can label the total "at least" rather than claim it is exact),
/// and computes `avg_latency_ms` as the `request_count`-weighted average
/// across rows (not a plain average of averages).
pub fn summarize(rows: &[AggregatedRow]) -> StatsSummary {
    let mut total_requests: u64 = 0;
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut total_cost_usd = 0.0_f64;
    let mut any_cost_known = false;
    let mut any_cost_unknown = false;
    let mut latency_weighted_sum = 0.0_f64;
    let mut total_errors: u64 = 0;

    for row in rows {
        total_requests += row.request_count as u64;
        total_input_tokens += row.input_tokens;
        total_output_tokens += row.output_tokens;
        total_errors += row.error_count as u64;
        latency_weighted_sum += row.avg_latency_ms * row.request_count as f64;
        match row.estimated_cost_usd {
            Some(cost) => {
                total_cost_usd += cost;
                any_cost_known = true;
            }
            None => any_cost_unknown = true,
        }
    }

    let avg_latency_ms = if total_requests > 0 {
        latency_weighted_sum / total_requests as f64
    } else {
        0.0
    };
    let error_rate = if total_requests > 0 {
        total_errors as f64 / total_requests as f64
    } else {
        0.0
    };

    StatsSummary {
        total_requests,
        total_input_tokens,
        total_output_tokens,
        total_cost_usd: if any_cost_known { Some(total_cost_usd) } else { None },
        any_cost_unknown,
        avg_latency_ms,
        total_errors,
        error_rate,
    }
}

/// Per-bucket totals (input + output tokens, summed across every
/// provider/model/kind in that bucket), sorted by bucket key ascending --
/// the data the bar chart renders.
pub fn bucket_totals(rows: &[AggregatedRow]) -> Vec<(String, u64)> {
    use std::collections::BTreeMap;
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    for row in rows {
        *totals.entry(row.bucket.clone()).or_insert(0) += row.input_tokens + row.output_tokens;
    }
    totals.into_iter().collect()
}

fn format_cost(summary: &StatsSummary) -> String {
    match summary.total_cost_usd {
        Some(cost) if summary.any_cost_unknown => format!("at least ${cost:.2}"),
        Some(cost) => format!("${cost:.2}"),
        None => "Unknown".to_string(),
    }
}

// ---------------------------------------------------------------------
// NexusStatsView
// ---------------------------------------------------------------------

struct StatCard {
    root: gtk::Box,
    /// Not read back directly -- `card_value_label` re-queries it from the
    /// widget tree via `summary_grid.child_at`/`first_child` instead, so a
    /// card's value can be updated without this struct having to be kept
    /// around anywhere. Kept as a field (rather than dropped immediately)
    /// so `build_stat_card`'s return value documents what it built.
    #[allow(dead_code)]
    value_label: gtk::Label,
}

fn build_stat_card(title: &str) -> StatCard {
    let value_label = gtk::Label::builder()
        .label("-")
        .css_classes(["title-1"])
        .xalign(0.0)
        .build();
    let title_label = gtk::Label::builder()
        .label(title)
        .css_classes(["dim-label", "caption"])
        .xalign(0.0)
        .build();
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .css_classes(["card"])
        .margin_start(12)
        .margin_end(12)
        .margin_top(10)
        .margin_bottom(10)
        .build();
    root.append(&value_label);
    root.append(&title_label);
    StatCard { root, value_label }
}

mod imp {
    use super::*;

    #[derive(gtk::CompositeTemplate)]
    #[template(resource = "/io/github/GITHUB_OWNER/Nexus/ui/stats-view.ui")]
    pub struct StatsView {
        #[template_child]
        pub granularity_day: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub granularity_week: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub granularity_month: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub preset_7_days: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub preset_30_days: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub preset_all_time: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub provider_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub model_dropdown: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub export_csv_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub summary_grid: TemplateChild<gtk::Grid>,
        #[template_child]
        pub bars_box: TemplateChild<gtk::Box>,

        pub rows: RefCell<Vec<AggregatedRow>>,
        pub provider_options: RefCell<Vec<String>>,
        pub model_options: RefCell<Vec<String>>,
    }

    impl Default for StatsView {
        fn default() -> Self {
            Self {
                granularity_day: TemplateChild::default(),
                granularity_week: TemplateChild::default(),
                granularity_month: TemplateChild::default(),
                preset_7_days: TemplateChild::default(),
                preset_30_days: TemplateChild::default(),
                preset_all_time: TemplateChild::default(),
                provider_dropdown: TemplateChild::default(),
                model_dropdown: TemplateChild::default(),
                export_csv_button: TemplateChild::default(),
                summary_grid: TemplateChild::default(),
                bars_box: TemplateChild::default(),
                rows: RefCell::new(Vec::new()),
                provider_options: RefCell::new(Vec::new()),
                model_options: RefCell::new(Vec::new()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StatsView {
        const NAME: &'static str = "NexusStatsView";
        type Type = super::StatsView;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for StatsView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_cards();
            self.obj().setup_signals();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("filters-changed").build()])
        }
    }

    impl WidgetImpl for StatsView {}
    impl BoxImpl for StatsView {}
}

glib::wrapper! {
    pub struct StatsView(ObjectSubclass<imp::StatsView>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for StatsView {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsView {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_rows(&self, rows: &[AggregatedRow]) {
        *self.imp().rows.borrow_mut() = rows.to_vec();
        self.refresh_cards();
        self.refresh_bars();
    }

    pub fn set_provider_options(&self, providers: &[String]) {
        *self.imp().provider_options.borrow_mut() = providers.to_vec();
        let mut items = vec!["All Providers".to_string()];
        items.extend(providers.iter().cloned());
        let model = gtk::StringList::new(&items.iter().map(String::as_str).collect::<Vec<_>>());
        self.imp().provider_dropdown.set_model(Some(&model));
    }

    pub fn set_model_options(&self, models: &[String]) {
        *self.imp().model_options.borrow_mut() = models.to_vec();
        let mut items = vec!["All Models".to_string()];
        items.extend(models.iter().cloned());
        let model = gtk::StringList::new(&items.iter().map(String::as_str).collect::<Vec<_>>());
        self.imp().model_dropdown.set_model(Some(&model));
    }

    pub fn granularity(&self) -> BucketGranularity {
        let imp = self.imp();
        if imp.granularity_week.is_active() {
            BucketGranularity::Week
        } else if imp.granularity_month.is_active() {
            BucketGranularity::Month
        } else {
            BucketGranularity::Day
        }
    }

    pub fn date_preset(&self) -> DatePreset {
        let imp = self.imp();
        if imp.preset_7_days.is_active() {
            DatePreset::Last7Days
        } else if imp.preset_30_days.is_active() {
            DatePreset::Last30Days
        } else {
            DatePreset::AllTime
        }
    }

    pub fn selected_provider(&self) -> Option<String> {
        let idx = self.imp().provider_dropdown.selected();
        if idx == 0 || idx == gtk::INVALID_LIST_POSITION {
            None
        } else {
            self.imp()
                .provider_options
                .borrow()
                .get(idx as usize - 1)
                .cloned()
        }
    }

    pub fn selected_model(&self) -> Option<String> {
        let idx = self.imp().model_dropdown.selected();
        if idx == 0 || idx == gtk::INVALID_LIST_POSITION {
            None
        } else {
            self.imp()
                .model_options
                .borrow()
                .get(idx as usize - 1)
                .cloned()
        }
    }

    pub fn connect_filters_changed(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.connect_local("filters-changed", false, move |values| {
            let obj = values[0].get::<Self>().expect("filters-changed arg 0");
            f(&obj);
            None
        })
    }

    /// Connects `f` to the Export CSV button's `clicked` signal (returning
    /// its real `SignalHandlerId`, so callers can disconnect it), and
    /// -- after `f` runs -- performs the actual file-save itself (see
    /// `export_csv`). `f` is for the application layer's own bookkeeping
    /// (e.g. re-fetching fresher rows right before exporting via
    /// `set_rows`); the CSV write does not depend on it running at all.
    pub fn connect_export_requested(&self, f: impl Fn(&Self) + 'static) -> glib::SignalHandlerId {
        self.imp().export_csv_button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| {
                f(&view);
                view.export_csv();
            }
        ))
    }

    fn export_csv(&self) {
        let rows = self.imp().rows.borrow().clone();
        let dialog = gtk::FileDialog::builder()
            .title("Export Usage Statistics")
            .initial_name("nexus-usage.csv")
            .build();
        let root = self.root().and_then(|r| r.downcast::<gtk::Window>().ok());
        dialog.save(
            root.as_ref(),
            gio::Cancellable::NONE,
            move |result| {
                let Ok(file) = result else { return };
                let csv = match nexus_stats::csv_export::rows_to_csv(&rows) {
                    Ok(csv) => csv,
                    Err(err) => {
                        tracing::error!("failed to render CSV: {err}");
                        return;
                    }
                };
                if let Some(path) = file.path() {
                    if let Err(err) = std::fs::write(&path, csv) {
                        tracing::error!("failed to write {}: {err}", path.display());
                    }
                }
            },
        );
    }

    fn setup_cards(&self) {
        let imp = self.imp();
        let titles = [
            "Total Requests",
            "Total Tokens",
            "Estimated Cost",
            "Avg Latency",
            "Error Rate",
        ];
        for (index, title) in titles.iter().enumerate() {
            let card = build_stat_card(title);
            imp.summary_grid.attach(&card.root, index as i32, 0, 1, 1);
        }
    }

    fn card_value_label(&self, index: i32) -> Option<gtk::Label> {
        let child = self.imp().summary_grid.child_at(index, 0)?;
        let card = child.downcast::<gtk::Box>().ok()?;
        card.first_child()?.downcast::<gtk::Label>().ok()
    }

    fn refresh_cards(&self) {
        let rows = self.imp().rows.borrow();
        let summary = summarize(&rows);
        drop(rows);

        if let Some(label) = self.card_value_label(0) {
            label.set_text(&summary.total_requests.to_string());
        }
        if let Some(label) = self.card_value_label(1) {
            label.set_text(&(summary.total_input_tokens + summary.total_output_tokens).to_string());
        }
        if let Some(label) = self.card_value_label(2) {
            label.set_text(&format_cost(&summary));
        }
        if let Some(label) = self.card_value_label(3) {
            label.set_text(&format!("{:.0} ms", summary.avg_latency_ms));
        }
        if let Some(label) = self.card_value_label(4) {
            label.set_text(&format!("{:.1}%", summary.error_rate * 100.0));
        }
    }

    fn refresh_bars(&self) {
        let imp = self.imp();
        while let Some(child) = imp.bars_box.first_child() {
            imp.bars_box.remove(&child);
        }
        let rows = imp.rows.borrow();
        let totals = bucket_totals(&rows);
        drop(rows);

        if totals.is_empty() {
            let empty = super::new_empty_state(
                "x-office-spreadsheet-symbolic",
                "No Usage Data Yet",
                "Chat with a model or run an automation to see statistics here.",
            );
            empty.set_vexpand(false);
            imp.bars_box.append(&empty);
            return;
        }

        let max = totals.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);

        for (bucket, value) in totals {
            let height = ((value as f64 / max as f64) * 150.0).max(2.0) as i32;
            let bar = gtk::Box::builder()
                .width_request(24)
                .height_request(height)
                .valign(gtk::Align::End)
                .css_classes(["nexus-stats-bar"])
                .tooltip_text(format!("{bucket}: {value} tokens"))
                .build();
            imp.bars_box.append(&bar);
        }
    }

    fn setup_signals(&self) {
        let imp = self.imp();

        let granularity_buttons = [
            imp.granularity_day.clone(),
            imp.granularity_week.clone(),
            imp.granularity_month.clone(),
        ];
        bind_exclusive_toggle_group(&granularity_buttons, self);

        let preset_buttons = [
            imp.preset_7_days.clone(),
            imp.preset_30_days.clone(),
            imp.preset_all_time.clone(),
        ];
        bind_exclusive_toggle_group(&preset_buttons, self);

        imp.provider_dropdown.connect_selected_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.emit_by_name::<()>("filters-changed", &[])
        ));
        imp.model_dropdown.connect_selected_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.emit_by_name::<()>("filters-changed", &[])
        ));
    }
}

/// Enforces "exactly one active" across `buttons` by hand (see the note in
/// `stats-view.blp` on why `GtkToggleButton::set_group` is not used), and
/// emits `filters-changed` whenever the active button changes.
fn bind_exclusive_toggle_group(buttons: &[gtk::ToggleButton], view: &StatsView) {
    for (index, button) in buttons.iter().enumerate() {
        let others: Vec<gtk::ToggleButton> = buttons
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, b)| b.clone())
            .collect();
        button.connect_toggled(glib::clone!(
            #[weak(rename_to = view)]
            view,
            #[strong]
            others,
            move |btn| {
                if btn.is_active() {
                    for other in &others {
                        other.set_active(false);
                    }
                    view.emit_by_name::<()>("filters-changed", &[]);
                } else if others.iter().all(|o| !o.is_active()) {
                    // Don't allow the last active button to be switched off.
                    btn.set_active(true);
                }
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_row(bucket: &str, cost: Option<f64>, requests: u32, latency: f64, errors: u32) -> AggregatedRow {
        AggregatedRow {
            bucket: bucket.to_string(),
            provider_id: "openai".to_string(),
            model_id: "gpt-5".to_string(),
            kind: "chat".to_string(),
            request_count: requests,
            input_tokens: 100,
            output_tokens: 50,
            estimated_cost_usd: cost,
            avg_latency_ms: latency,
            error_count: errors,
        }
    }

    #[test]
    fn summarize_sums_and_weights_correctly() {
        let rows = vec![
            sample_row("2026-01-01", Some(1.0), 10, 100.0, 1),
            sample_row("2026-01-02", Some(2.0), 30, 200.0, 0),
        ];
        let summary = summarize(&rows);
        assert_eq!(summary.total_requests, 40);
        assert_eq!(summary.total_input_tokens, 200);
        assert_eq!(summary.total_output_tokens, 100);
        assert_eq!(summary.total_cost_usd, Some(3.0));
        assert!(!summary.any_cost_unknown);
        // Weighted average: (100*10 + 200*30) / 40 = 175.0
        assert!((summary.avg_latency_ms - 175.0).abs() < 1e-9);
        assert_eq!(summary.total_errors, 1);
        assert!((summary.error_rate - 0.025).abs() < 1e-9);
    }

    #[test]
    fn summarize_flags_unknown_cost_and_still_sums_known_ones() {
        let rows = vec![
            sample_row("2026-01-01", Some(1.0), 10, 100.0, 0),
            sample_row("2026-01-02", None, 10, 100.0, 0),
        ];
        let summary = summarize(&rows);
        assert_eq!(summary.total_cost_usd, Some(1.0));
        assert!(summary.any_cost_unknown);
    }

    #[test]
    fn summarize_cost_is_none_when_no_row_has_a_known_price() {
        let rows = vec![sample_row("2026-01-01", None, 10, 100.0, 0)];
        let summary = summarize(&rows);
        assert_eq!(summary.total_cost_usd, None);
        assert!(summary.any_cost_unknown);
    }

    #[test]
    fn summarize_empty_rows_do_not_divide_by_zero() {
        let summary = summarize(&[]);
        assert_eq!(summary.total_requests, 0);
        assert_eq!(summary.avg_latency_ms, 0.0);
        assert_eq!(summary.error_rate, 0.0);
    }

    #[test]
    fn bucket_totals_sums_across_rows_in_same_bucket_sorted_by_key() {
        let rows = vec![
            sample_row("2026-01-02", Some(1.0), 10, 100.0, 0),
            sample_row("2026-01-01", Some(1.0), 10, 100.0, 0),
            sample_row("2026-01-01", Some(1.0), 10, 100.0, 0),
        ];
        let totals = bucket_totals(&rows);
        assert_eq!(totals, vec![
            ("2026-01-01".to_string(), 300),
            ("2026-01-02".to_string(), 150),
        ]);
    }

    #[test]
    fn date_preset_range_last_7_days_is_seven_days_before_now() {
        let now = Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
        let (since, until) = date_preset_range(DatePreset::Last7Days, now);
        assert_eq!(since, Some(now - Duration::days(7)));
        assert_eq!(until, None);
    }

    #[test]
    fn date_preset_range_all_time_has_no_bounds() {
        let now = Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
        assert_eq!(date_preset_range(DatePreset::AllTime, now), (None, None));
    }
}
