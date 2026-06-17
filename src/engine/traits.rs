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

    /// The widget to embed in the window's view stack.
    fn widget(&self) -> gtk4::Widget;
}
