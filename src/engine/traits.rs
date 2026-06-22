//! Engine abstraction traits.
//!
//! These decouple the effect runner and UI from the concrete web engine. The
//! only GTK type referenced is [`gtk4::Widget`], which is the windowing surface a
//! view embeds into and is independent of which web engine renders it.

/// A single engine-backed web view.
pub trait EngineView {
    /// Load a URI.
    fn load_uri(&self, uri: &str);

    /// Reload, optionally bypassing the cache.
    fn reload(&self, bypass_cache: bool);

    /// Stop loading.
    fn stop(&self);

    /// Navigate back.
    fn go_back(&self);

    /// Navigate forward.
    fn go_forward(&self);

    /// Evaluate JavaScript; the result (or error) is delivered to `on_done`.
    fn evaluate_js(&self, script: &str, on_done: Box<dyn FnOnce(Result<String, String>)>);

    /// Set the page zoom level (1.0 = 100%).
    fn set_zoom(&self, level: f64);

    /// Enable or disable JavaScript for this view. Takes effect on the next load.
    fn set_javascript_enabled(&self, enabled: bool);

    /// Search the page for `text`, moving to the first match.
    fn find(&self, text: &str);

    /// Move to the next match, wrapping at the end.
    fn find_next(&self);

    /// Move to the previous match. Best-effort: WebKit's backward search is
    /// unreliable (cannot reach hidden/collapsed content and corrupts the find
    /// controller's heap, surfacing as a harmless error at process exit).
    fn find_previous(&self);

    /// Clear the current search highlight.
    fn find_clear(&self);

    /// The widget to embed in the window's view stack.
    fn widget(&self) -> gtk4::Widget;
}
