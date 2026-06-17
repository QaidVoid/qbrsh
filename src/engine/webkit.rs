//! WebKitGTK 6 implementation of the engine traits.
//!
//! This is the only module that references `webkit6` types. It builds web views,
//! wires their signals to [`Msg`] values on the mailbox, and exposes operations
//! through [`EngineView`].

use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{HardwareAccelerationPolicy, NetworkSession, Settings, WebView};

use crate::core::command::{Command, OpenTarget};
use crate::core::msg::{LoadEvent, Msg};
use crate::core::runtime::Mailbox;
use crate::core::state::TabId;

use super::traits::EngineView;

/// Factory that builds web views sharing one settings object and network session.
pub struct WebKitEngine {
    settings: Settings,
    session: NetworkSession,
}

impl WebKitEngine {
    /// Create the engine with default settings. `debug` enables developer tools
    /// and console output to stdout.
    pub fn new(debug: bool) -> Self {
        Self {
            settings: default_settings(debug),
            session: NetworkSession::default().expect("default network session"),
        }
    }

    /// Build a view for `tab`, wiring its signals to messages on `mailbox`.
    pub fn create_view(&self, tab: TabId, mailbox: Mailbox) -> Box<dyn EngineView> {
        let view = WebView::builder()
            .settings(&self.settings)
            .network_session(&self.session)
            .build();
        view.set_vexpand(true);
        view.set_hexpand(true);

        connect_signals(&view, tab, mailbox);
        Box::new(WebKitView { view })
    }
}

/// Wire a view's WebKit signals to messages.
fn connect_signals(view: &WebView, tab: TabId, mailbox: Mailbox) {
    let mb = mailbox.clone();
    view.connect_load_changed(move |_v, event| {
        let event = match event {
            webkit6::LoadEvent::Started => LoadEvent::Started,
            webkit6::LoadEvent::Committed => LoadEvent::Committed,
            webkit6::LoadEvent::Finished => LoadEvent::Finished,
            _ => return,
        };
        mb.send(Msg::Load { tab, event });
    });

    let mb = mailbox.clone();
    view.connect_uri_notify(move |v| {
        if let Some(uri) = v.uri() {
            mb.send(Msg::UriChanged {
                tab,
                uri: uri.to_string(),
            });
        }
    });

    let mb = mailbox.clone();
    view.connect_title_notify(move |v| {
        mb.send(Msg::TitleChanged {
            tab,
            title: v.title().map(|t| t.to_string()).unwrap_or_default(),
        });
    });

    let mb = mailbox.clone();
    view.connect_estimated_load_progress_notify(move |v| {
        mb.send(Msg::Progress {
            tab,
            fraction: v.estimated_load_progress(),
        });
    });

    let mb = mailbox.clone();
    view.connect_web_process_terminated(move |_v, _reason| {
        mb.send(Msg::Crashed { tab });
    });

    // New-window requests (target=_blank, window.open) open a foreground tab.
    let mb = mailbox.clone();
    view.connect_create(move |_v, nav_action| {
        if let Some(uri) = nav_action.request().and_then(|r| r.uri()) {
            mb.send(Msg::Command(Command::Open {
                target: OpenTarget::Tab,
                input: uri.to_string(),
            }));
        }
        None
    });
}

/// Shared WebKit settings for all views.
fn default_settings(debug: bool) -> Settings {
    let settings = Settings::new();
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Always);
    settings.set_enable_page_cache(true);
    settings.set_enable_smooth_scrolling(true);
    settings.set_enable_javascript(true);
    settings.set_enable_developer_extras(debug);
    settings.set_enable_write_console_messages_to_stdout(debug);
    settings
}

/// A WebKitGTK-backed view.
struct WebKitView {
    view: WebView,
}

impl EngineView for WebKitView {
    fn load_uri(&self, uri: &str) {
        WebViewExt::load_uri(&self.view, uri);
    }

    fn reload(&self, bypass_cache: bool) {
        if bypass_cache {
            self.view.reload_bypass_cache();
        } else {
            WebViewExt::reload(&self.view);
        }
    }

    fn stop(&self) {
        WebViewExt::stop_loading(&self.view);
    }

    fn go_back(&self) {
        WebViewExt::go_back(&self.view);
    }

    fn go_forward(&self) {
        WebViewExt::go_forward(&self.view);
    }

    fn evaluate_js(&self, script: &str, on_done: Box<dyn FnOnce(Result<String, String>)>) {
        self.view.evaluate_javascript(
            script,
            None,
            None::<&str>,
            None::<&gtk4::gio::Cancellable>,
            move |result| {
                on_done(match result {
                    Ok(value) => Ok(value.to_str().to_string()),
                    Err(e) => Err(e.to_string()),
                });
            },
        );
    }

    fn widget(&self) -> gtk4::Widget {
        self.view.clone().upcast::<gtk4::Widget>()
    }
}
