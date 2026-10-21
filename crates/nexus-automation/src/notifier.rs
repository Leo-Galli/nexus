/// Decouples the engine from GTK: the application implements this using
/// `gio::Notification`, which is routed through the Notification portal
/// automatically when sandboxed.
pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str, body: &str);
}

/// Used in tests and anywhere a `Notify` step should be a silent no-op.
pub struct NullNotifier;

impl Notifier for NullNotifier {
    fn notify(&self, _title: &str, _body: &str) {}
}
