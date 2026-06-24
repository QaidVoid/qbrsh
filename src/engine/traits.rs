//! Engine abstraction traits.
//!
//! These decouple the effect runner and UI from the concrete web engine. The
//! only GTK type referenced is [`gtk4::Widget`], which is the windowing surface a
//! view embeds into and is independent of which web engine renders it.

/// A single engine-backed web view.
pub trait EngineView {
    /// Load a URI.
    fn load_uri(&self, uri: &str);

    /// Render a generated HTML document directly in the view (used for
    /// view-source, where the source is shown as a generated page).
    fn load_html(&self, html: &str);

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

    /// Open the system print dialog for this page.
    fn print(&self);

    /// Save this page to the downloads directory as a web archive (MHTML). The
    /// write runs asynchronously; on completion `on_done` receives the saved
    /// path on success or an error message on failure.
    fn save_mhtml(&self, on_done: Box<dyn FnOnce(Result<String, String>)>);

    /// Toggle the Web Inspector for this view: show it when hidden, close it
    /// when shown.
    fn toggle_inspector(&self);

    /// The widget to embed in the window's view stack.
    fn widget(&self) -> gtk4::Widget;
}
