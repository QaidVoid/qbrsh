//! WebKitGTK 6 implementation of the engine traits.
//!
//! This is the only module that references `webkit6` types. It builds web views,
//! wires their signals to [`Msg`] values on the mailbox, and exposes operations
//! through [`EngineView`].

use std::collections::HashSet;
use std::rc::Rc;

use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    HardwareAccelerationPolicy, NavigationPolicyDecision, NetworkSession, PolicyDecisionType,
    Settings, UserContentInjectedFrames, UserScript, UserScriptInjectionTime, WebView,
};

use crate::adblock;
use crate::core::command::{Command, OpenTarget};
use crate::core::msg::{LoadEvent, Msg};
use crate::core::runtime::Mailbox;
use crate::core::state::TabId;

use super::traits::EngineView;

/// Factory that builds web views sharing one settings object and network session.
pub struct WebKitEngine {
    settings: Settings,
    session: NetworkSession,
    blocklist: Rc<HashSet<String>>,
}

impl WebKitEngine {
    /// Create the engine with default settings and the given ad blocklist.
    /// `debug` enables developer tools and console output to stdout.
    pub fn new(debug: bool, blocklist: Rc<HashSet<String>>) -> Self {
        Self {
            settings: default_settings(debug),
            session: NetworkSession::default().expect("default network session"),
            blocklist,
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

        connect_signals(&view, tab, mailbox, self.blocklist.clone());
        Box::new(WebKitView { view })
    }
}

/// Wire a view's WebKit signals to messages.
fn connect_signals(view: &WebView, tab: TabId, mailbox: Mailbox, blocklist: Rc<HashSet<String>>) {
    // Block navigations and subframe loads to ad/tracker domains. This runs
    // synchronously and natively, never through the message loop (design D5).
    view.connect_decide_policy(move |_v, decision, decision_type| {
        if matches!(
            decision_type,
            PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction
        ) && let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>()
            && let Some(uri) = nav
                .navigation_action()
                .and_then(|a| a.request())
                .and_then(|r| r.uri())
            && adblock::is_blocked(&uri, &blocklist)
        {
            decision.ignore();
            return true;
        }
        false
    });

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

    // Deny permission requests (geolocation, notifications, media) by default.
    // Per-site allow lists are a future config addition. Script dialogs
    // (alert/confirm/prompt) use WebKit's built-in handling.
    view.connect_permission_request(|_v, request| {
        request.deny();
        true
    });

    // Show a styled error page when a load fails (but not when we cancelled it,
    // e.g. a new-window request handed off to a tab).
    view.connect_load_failed(|v, _event, uri, error| {
        if error.matches(webkit6::NetworkError::Cancelled) {
            return false;
        }
        v.load_html(&error_page_html(uri, &error.to_string()), Some(uri));
        true
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

    // Auto-switch between Insert and Normal mode as editable elements gain or
    // lose focus, via a content script that posts to a message handler.
    let ucm = view
        .user_content_manager()
        .expect("web view has a user content manager");
    ucm.add_script(&UserScript::new(
        INSERT_MODE_DETECT_JS,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::End,
        &[],
        &[],
    ));
    ucm.add_script(&UserScript::new(
        HINTS_JS,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::End,
        &[],
        &[],
    ));
    ucm.register_script_message_handler("qbrshMode", None);
    let mb = mailbox.clone();
    ucm.connect_script_message_received(Some("qbrshMode"), move |_ucm, value| {
        let focused = match value.to_str().as_str() {
            "insert" => true,
            "normal" => false,
            _ => return,
        };
        mb.send(Msg::InputFocusChanged { tab, focused });
    });
}

/// Hint-mode engine injected into every page (defines `window.__qbrshHints`).
const HINTS_JS: &str = include_str!("../../js/hints.js");

/// Content script that reports focus on editable elements for auto-insert-mode.
const INSERT_MODE_DETECT_JS: &str = r#"(function(){
  function editable(el){
    if(!el) return false;
    var t=(el.tagName||'').toLowerCase();
    if(t==='input'){
      var ty=(el.type||'text').toLowerCase();
      return !['button','submit','reset','checkbox','radio','file','image','hidden','range','color'].includes(ty);
    }
    return t==='textarea'||t==='select'||el.isContentEditable;
  }
  document.addEventListener('focusin',function(e){if(editable(e.target))window.webkit.messageHandlers.qbrshMode.postMessage('insert');},true);
  document.addEventListener('focusout',function(e){if(editable(e.target))window.webkit.messageHandlers.qbrshMode.postMessage('normal');},true);
})();"#;

/// Render a minimal error page for a failed load.
fn error_page_html(uri: &str, error: &str) -> String {
    let uri = html_escape(uri);
    let error = html_escape(error);
    format!(
        r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Load failed</title>
<style>body{{background:#1a1a2e;color:#e0e0e0;font-family:system-ui,sans-serif;text-align:center;padding:4em 2em}}
h1{{color:#e06c75}}code{{background:#2a2a4a;padding:3px 8px;border-radius:4px;word-break:break-all}}
.err{{color:#e5c07b;margin-top:1em}}.hint{{color:#888;margin-top:2.5em;font-size:.9em}}</style></head>
<body><h1>Page load failed</h1><p><code>{uri}</code></p><p class="err">{error}</p>
<p class="hint">Press <b>r</b> to retry or <b>H</b> to go back.</p></body></html>"#
    )
}

/// Escape the few characters that matter when embedding text in HTML.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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
