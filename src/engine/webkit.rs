//! WebKitGTK 6 implementation of the engine traits.
//!
//! This is the only module that references `webkit6` types. It builds web views,
//! wires their signals to [`Msg`] values on the mailbox, and exposes operations
//! through [`EngineView`].

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    CacheModel, Download, FindOptions, GeolocationPermissionRequest, HardwareAccelerationPolicy,
    MemoryPressureSettings, NavigationPolicyDecision, NetworkSession,
    NotificationPermissionRequest, PermissionRequest, PolicyDecisionType, ResponsePolicyDecision,
    Settings, TLSErrorsPolicy, UserContentFilter, UserContentFilterStore,
    UserContentInjectedFrames, UserMediaPermissionRequest, UserScript, UserScriptInjectionTime,
    WebContext, WebView,
};

use crate::adblock;
use crate::core::command::{Command, OpenTarget, is_unsafe_open_target};
use crate::core::msg::{LoadEvent, Msg};
use crate::core::runtime::Mailbox;
use crate::core::state::{Capability, PermissionPolicy, Permissions, TabId};

/// Deferred permission requests awaiting the user's decision, keyed by id.
type PendingPermissions = Rc<RefCell<HashMap<u64, PermissionRequest>>>;

use super::traits::EngineView;

/// Shared, runtime-updatable permission policy read by the permission handler.
pub type PermissionMirror = Rc<RefCell<Permissions>>;

/// Live download handles held by the engine for cancel, keyed by id.
type DownloadMap = Rc<RefCell<HashMap<u64, Download>>>;
/// Ids of user-cancelled downloads, so their in-flight failure is reported as a
/// cancellation rather than an error.
type CancelledDownloads = Rc<RefCell<HashSet<u64>>>;

/// Upper bound on in-page matches counted/highlighted per search. Capped rather
/// than unbounded because an unbounded limit has crashed WebKit's find
/// controller on pages with very many matches.
const MAX_FIND_MATCHES: u32 = 1000;

/// Registry of created views, each weakly held and tagged with its creation-time
/// site key for same-site web-process grouping.
type ViewRegistry = Rc<RefCell<Vec<(Option<String>, glib::object::WeakRef<WebView>)>>>;

/// Factory that builds web views sharing one settings object and network session.
pub struct WebKitEngine {
    settings: Settings,
    /// Web context shared by all views; carries the cache model and web-process
    /// memory-pressure settings. Views relate to a live sibling and inherit it.
    context: WebContext,
    session: NetworkSession,
    blocklist: Rc<HashSet<String>>,
    permissions: PermissionMirror,
    /// Kept alive for the duration of the engine; backs the compiled filter.
    _filter_store: UserContentFilterStore,
    /// The compiled subresource content filter, once compilation finishes.
    filter: Rc<RefCell<Option<UserContentFilter>>>,
    /// Weak references to created views, each tagged with its creation-time site
    /// key, used to relate a new view to a live same-site view (sharing a web
    /// process) and to apply the content filter once it compiles. Weak so closed
    /// tabs drop out and the registry stays bounded.
    views: ViewRegistry,
    /// Permission requests deferred for an interactive prompt, resolved later by
    /// id from the user's decision.
    pending_permissions: PendingPermissions,
    /// Allocates ids for deferred permission requests.
    next_permission_id: Rc<Cell<u64>>,
    /// Active downloads held for cancel, keyed by id; dropped on terminal state.
    downloads: DownloadMap,
    /// Ids whose in-flight failure should be reported as a user cancellation.
    cancelled_downloads: CancelledDownloads,
}

impl WebKitEngine {
    /// Create the engine with default settings, the ad blocklist, and the
    /// permission policy mirror. Compiles a subresource content filter from the
    /// blocklist asynchronously into `filter_store_dir`. `debug` enables
    /// developer tools and console output.
    pub fn new(
        debug: bool,
        blocklist: Rc<HashSet<String>>,
        permissions: PermissionMirror,
        filter_store_dir: &Path,
        downloads_dir: &Path,
        mailbox: Mailbox,
    ) -> Self {
        let _ = std::fs::create_dir_all(filter_store_dir);
        let store = UserContentFilterStore::new(&filter_store_dir.to_string_lossy());
        let filter: Rc<RefCell<Option<UserContentFilter>>> = Rc::new(RefCell::new(None));
        let views: ViewRegistry = Rc::new(RefCell::new(Vec::new()));

        let json = adblock::content_filter_json(&blocklist);
        let bytes = glib::Bytes::from(json.as_bytes());
        let filter_cb = filter.clone();
        let views_cb = views.clone();
        store.save(
            "qbrsh-adblock",
            &bytes,
            None::<&gtk4::gio::Cancellable>,
            move |result| match result {
                Ok(compiled) => {
                    for view in views_cb.borrow().iter().filter_map(|(_, w)| w.upgrade()) {
                        if let Some(ucm) = view.user_content_manager() {
                            ucm.add_filter(&compiled);
                        }
                    }
                    *filter_cb.borrow_mut() = Some(compiled);
                }
                Err(e) => eprintln!("[qbrsh] content filter compile failed: {e}"),
            },
        );

        // Bound renderer memory: a browser-appropriate but capped cache model on
        // the shared context, plus memory-pressure handling on both the web and
        // network processes so they release memory under load. The network
        // process must be configured before its session is created.
        let context = WebContext::builder()
            .memory_pressure_settings(&MemoryPressureSettings::new())
            .build();
        context.set_cache_model(CacheModel::DocumentBrowser);

        let mut net_pressure = MemoryPressureSettings::new();
        NetworkSession::set_memory_pressure_settings(&mut net_pressure);

        let session = NetworkSession::default().expect("default network session");
        // Pin the secure default so it cannot silently regress: certificate
        // errors block the load rather than being ignored.
        session.set_tls_errors_policy(TLSErrorsPolicy::Fail);

        // Save downloads to the downloads directory with a safe, de-duplicated
        // name, reporting lifecycle events as messages.
        let dl_dir = downloads_dir.to_path_buf();
        let dl_mailbox = mailbox.clone();
        let dl_id = Rc::new(Cell::new(0u64));
        let dl_map: DownloadMap = Rc::new(RefCell::new(HashMap::new()));
        let dl_cancelled: CancelledDownloads = Rc::new(RefCell::new(HashSet::new()));
        // Clones moved into the per-download-started closure; the originals are
        // kept for the engine struct (cancel/retry).
        let dl_started_map = dl_map.clone();
        let dl_started_cancelled = dl_cancelled.clone();
        session.connect_download_started(move |_session, download| {
            let id = dl_id.get();
            dl_id.set(id + 1);
            // The original request URI is available now; it is the source used
            // for retry and surfaced to core alongside the chosen destination.
            let source = download
                .request()
                .and_then(|r| r.uri())
                .map(|s| s.to_string())
                .unwrap_or_default();
            wire_download(
                download,
                id,
                source,
                dl_dir.clone(),
                dl_mailbox.clone(),
                dl_started_map.clone(),
                dl_started_cancelled.clone(),
            );
        });

        Self {
            settings: default_settings(debug),
            context,
            session,
            blocklist,
            permissions,
            _filter_store: store,
            filter,
            views,
            pending_permissions: Rc::new(RefCell::new(HashMap::new())),
            next_permission_id: Rc::new(Cell::new(0)),
            downloads: dl_map,
            cancelled_downloads: dl_cancelled,
        }
    }

    /// Resolve a deferred permission request by id from the user's decision.
    /// A missing id (already resolved, or its tab closed) is a no-op.
    pub fn resolve_permission(&self, id: u64, allow: bool) {
        if let Some(request) = self.pending_permissions.borrow_mut().remove(&id) {
            if allow {
                request.allow();
            } else {
                request.deny();
            }
        }
    }

    /// Cancel an in-flight download by id. Records the id as user-cancelled so
    /// the engine reports a cancellation (not a failure) when WebKit aborts. A
    /// missing id (already finished, failed, or cancelled) is a no-op.
    pub fn cancel_download(&self, id: u64) {
        if let Some(dl) = self.downloads.borrow_mut().remove(&id) {
            self.cancelled_downloads.borrow_mut().insert(id);
            dl.cancel();
        }
    }

    /// Re-issue a download for `source`, going through the normal download-started
    /// path so it gets a fresh id and record. Used to retry a failed download.
    pub fn retry_download(&self, source: &str) {
        self.session.download_uri(source);
    }

    /// Build a view for `tab` loading `uri`, wiring its signals to messages on
    /// `mailbox`. The view shares a web process only with a live same-site view,
    /// so different sites stay in separate renderer processes.
    pub fn create_view(&self, tab: TabId, uri: &str, mailbox: Mailbox) -> Box<dyn EngineView> {
        let site = adblock::site_of(uri);
        // Prune dead references and relate only to a live view of the same site.
        let related = {
            let mut views = self.views.borrow_mut();
            views.retain(|(_, w)| w.upgrade().is_some());
            site.as_deref().and_then(|s| {
                views
                    .iter()
                    .find(|(vs, _)| vs.as_deref() == Some(s))
                    .and_then(|(_, w)| w.upgrade())
            })
        };

        let builder = WebView::builder()
            .settings(&self.settings)
            .network_session(&self.session);
        // A same-site related view shares its sibling's web process and context;
        // an unrelated (new-site) view sets the shared context explicitly and
        // gets its own process.
        let builder = match related {
            Some(ref r) => builder.related_view(r),
            None => builder.web_context(&self.context),
        };
        let view = builder.build();
        view.set_vexpand(true);
        view.set_hexpand(true);

        // Apply the subresource content filter now if it is ready; otherwise the
        // compile callback applies it to every live view (this one included).
        if let Some(ucm) = view.user_content_manager()
            && let Some(compiled) = self.filter.borrow().as_ref()
        {
            ucm.add_filter(compiled);
        }
        self.views.borrow_mut().push((site, view.downgrade()));

        connect_signals(
            &view,
            tab,
            mailbox,
            self.session.clone(),
            self.blocklist.clone(),
            self.permissions.clone(),
            self.pending_permissions.clone(),
            self.next_permission_id.clone(),
        );
        Box::new(WebKitView { view })
    }
}

/// Wire a view's WebKit signals to messages.
#[allow(clippy::too_many_arguments)]
fn connect_signals(
    view: &WebView,
    tab: TabId,
    mailbox: Mailbox,
    session: NetworkSession,
    blocklist: Rc<HashSet<String>>,
    permissions: PermissionMirror,
    pending_permissions: PendingPermissions,
    next_permission_id: Rc<Cell<u64>>,
) {
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
        // Route responses WebKit cannot display (application/octet-stream,
        // archives, etc.) to a download. Converting the in-flight main-frame
        // response with decision.download() can stall (the body never streams
        // after the page load is abandoned), so instead ignore the page load and
        // start a standalone download for the request URI. The download is
        // started from an idle callback, not synchronously here: emitting a fresh
        // download (and its signals) from inside this policy handler re-enters
        // WebKit and leaves the download stuck. Displayable types fall through.
        if decision_type == PolicyDecisionType::Response
            && let Some(resp) = decision.downcast_ref::<ResponsePolicyDecision>()
            && !resp.is_mime_type_supported()
        {
            if let Some(uri) = resp.request().and_then(|r| r.uri()).map(|s| s.to_string()) {
                decision.ignore();
                let session = session.clone();
                glib::MainContext::default().invoke_local(move || {
                    session.download_uri(&uri);
                });
                return true;
            }
            decision.download();
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

    // Resolve permission requests (geolocation, notifications, media) from the
    // per-site, per-capability policy keyed by the view's current host. An `ask`
    // policy defers the request to the interactive prompt (resolved later by id).
    // Script dialogs use WebKit's built-in handling.
    let mb = mailbox.clone();
    view.connect_permission_request(move |v, request| {
        let host = v
            .uri()
            .and_then(|u| adblock::host_of(&u).map(str::to_string))
            .unwrap_or_default();
        // Classify into a supported capability; unsupported request types
        // (clipboard, pointer-lock, device-info, ...) are denied.
        let Some(capability) = classify_permission(request) else {
            request.deny();
            return true;
        };
        match permissions.borrow().policy_for(&host, capability) {
            PermissionPolicy::Allow => request.allow(),
            PermissionPolicy::Deny => request.deny(),
            // Defer: hold the request and ask the user, resolving later by id.
            PermissionPolicy::Ask => {
                let id = next_permission_id.get();
                next_permission_id.set(id + 1);
                pending_permissions.borrow_mut().insert(id, request.clone());
                mb.send(Msg::PermissionRequested {
                    id,
                    host,
                    capability,
                });
            }
        }
        true
    });

    // Report in-page search results. The total comes from the dedicated
    // counting pass (counted-matches); found-text only confirms a hit and its
    // count is unreliable, so it is not used for the total.
    if let Some(fc) = view.find_controller() {
        let mb = mailbox.clone();
        fc.connect_counted_matches(move |_fc, count| {
            mb.send(Msg::FindResult {
                tab,
                matches: count,
            });
        });
        let mb = mailbox.clone();
        fc.connect_failed_to_find_text(move |_fc| {
            mb.send(Msg::FindResult { tab, matches: 0 });
        });
    }

    // Show a styled error page when a load fails (but not when we cancelled it,
    // e.g. a new-window request handed off to a tab).
    // Show a styled error page when a load fails, but not when we caused the
    // interruption ourselves: a navigation converted to a download (or otherwise
    // interrupted by a policy change) is reported as a load failure, and a
    // cancelled load is expected. Neither should clobber the current page.
    view.connect_load_failed(|v, _event, uri, error| {
        if error.matches(webkit6::NetworkError::Cancelled)
            || error.matches(webkit6::PolicyError::FrameLoadInterruptedByPolicyChange)
        {
            return false;
        }
        v.load_html(&error_page_html(uri, &error.to_string()), Some(uri));
        true
    });

    // New-window requests (target=_blank, window.open) open a foreground tab.
    // Block only schemes that execute script or render arbitrary inline content
    // (data:, javascript:); file: and normal web schemes pass. WebKit itself
    // already gates cross-origin web->file: access, so we do not block file:.
    let mb = mailbox.clone();
    view.connect_create(move |_v, nav_action| {
        if let Some(uri) = nav_action.request().and_then(|r| r.uri()) {
            if is_unsafe_open_target(&uri) {
                eprintln!("[qbrsh] blocked new-window open to unsafe target: {uri}");
            } else {
                mb.send(Msg::Command(Command::Open {
                    target: OpenTarget::Tab,
                    input: uri.to_string(),
                }));
            }
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

/// Wire a started download: hold its handle for cancel, then choose a safe
/// destination and report start, live progress, completion, cancellation, and
/// failure as messages.
fn wire_download(
    download: &Download,
    id: u64,
    source: String,
    dir: PathBuf,
    mailbox: Mailbox,
    map: DownloadMap,
    cancelled: CancelledDownloads,
) {
    // Hold the handle so a user cancel can reach it; dropped on terminal state.
    map.borrow_mut().insert(id, download.clone());

    let started = mailbox.clone();
    download.connect_decide_destination(move |dl, suggested| {
        let dest = safe_download_path(&dir, suggested);
        let filename = dest
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("download")
            .to_string();
        let path = dest.to_string_lossy().to_string();
        // set_destination requires a plain absolute path: GLib asserts
        // g_path_is_absolute(destination), so a file:// URI is rejected and the
        // download stalls. Core keeps this same path for display/open.
        dl.set_destination(&path);
        started.send(Msg::DownloadStarted {
            id,
            filename,
            path,
            source: source.clone(),
        });
        true
    });

    // Report progress, throttled to one message per 1% of the total (or 64 KiB
    // when the total is unknown) so the dispatch loop is not flooded per chunk.
    let progress = mailbox.clone();
    let last_reported = Rc::new(Cell::new(0u64));
    let last = last_reported.clone();
    download.connect_received_data(move |dl, _| {
        let received = dl.received_data_length();
        let total = dl.response().map(|r| r.content_length()).unwrap_or(0);
        let prev = last.get();
        let step = if total > 0 {
            (total / 100).max(1)
        } else {
            64 * 1024
        };
        if received >= prev.saturating_add(step) || (total > 0 && received >= total) {
            last.set(received);
            progress.send(Msg::DownloadProgress {
                id,
                received,
                total,
            });
        }
    });

    let finished = mailbox.clone();
    let fin_map = map.clone();
    download.connect_finished(move |_dl| {
        fin_map.borrow_mut().remove(&id);
        finished.send(Msg::DownloadFinished { id });
    });

    let fail_map = map.clone();
    let fail_cancelled = cancelled.clone();
    download.connect_failed(move |_dl, error| {
        fail_map.borrow_mut().remove(&id);
        if fail_cancelled.borrow_mut().remove(&id) {
            mailbox.send(Msg::DownloadCancelled { id });
        } else {
            mailbox.send(Msg::DownloadFailed {
                id,
                error: error.to_string(),
            });
        }
    });
}

/// Compute a safe destination inside `dir` for `suggested`: a single sanitized
/// path component that cannot escape `dir`, de-duplicated against existing files.
fn safe_download_path(dir: &Path, suggested: &str) -> PathBuf {
    let name = sanitize_download_name(suggested);
    let mut candidate = dir.join(&name);
    let stem = Path::new(&name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download")
        .to_string();
    let ext = Path::new(&name)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_string);
    let mut n = 1;
    while candidate.exists() && n < 10_000 {
        let renamed = match &ext {
            Some(e) => format!("{stem}-{n}.{e}"),
            None => format!("{stem}-{n}"),
        };
        candidate = dir.join(renamed);
        n += 1;
    }
    candidate
}

/// Reduce a suggested filename to a single safe component (no separators, no
/// parent refs, no query/fragment), falling back to `download`.
fn sanitize_download_name(suggested: &str) -> String {
    // `suggested` may be a bare filename, a path, or (when the server gave none)
    // a URL. Drop any query or fragment first, since neither belongs in a name.
    let cleaned = suggested.split(['?', '#']).next().unwrap_or("");
    let base = Path::new(cleaned)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .trim();
    if base.is_empty() || base == "." || base == ".." {
        "download".to_string()
    } else {
        base.to_string()
    }
}

/// Map a WebKit permission request to a supported capability, or `None` for
/// request types the browser does not surface (which are denied). A combined
/// audio+video request is treated as the camera (the higher-risk capability).
fn classify_permission(request: &PermissionRequest) -> Option<Capability> {
    if request.is::<GeolocationPermissionRequest>() {
        Some(Capability::Geolocation)
    } else if request.is::<NotificationPermissionRequest>() {
        Some(Capability::Notifications)
    } else if let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() {
        if media.is_for_video_device() {
            Some(Capability::Camera)
        } else if media.is_for_audio_device() {
            Some(Capability::Microphone)
        } else {
            None
        }
    } else {
        None
    }
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

/// Escape the characters that matter when embedding text in HTML, including
/// quotes, so interpolated values stay inert even if moved into an attribute.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Shared WebKit settings for all views.
fn default_settings(debug: bool) -> Settings {
    let settings = Settings::new();
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Always);
    // The in-memory back/forward page cache holds whole pages resident; disabling
    // it trades slightly slower back/forward for a smaller footprint.
    settings.set_enable_page_cache(false);
    settings.set_enable_smooth_scrolling(true);
    settings.set_enable_javascript(true);
    // Enable getUserMedia / WebRTC; without media-stream the engine rejects
    // camera/mic requests before any permission request is emitted (no prompt).
    settings.set_enable_media_stream(true);
    settings.set_enable_webrtc(true);
    // Web inspector (right-click -> Inspect Element) is always available.
    settings.set_enable_developer_extras(true);
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

    fn set_zoom(&self, level: f64) {
        self.view.set_zoom_level(level);
    }

    fn find(&self, text: &str) {
        if let Some(fc) = self.view.find_controller() {
            // Always forward with wraparound. Backward search (the BACKWARDS
            // option and search_previous) corrupts the find controller's heap in
            // this WebKitGTK version, so it is not used; `n` wraps, keeping every
            // match reachable. Count first so the search options stay active, and
            // cap the match limit since an unbounded limit has also crashed it.
            let opts = (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits();
            fc.count_matches(text, FindOptions::CASE_INSENSITIVE.bits(), MAX_FIND_MATCHES);
            fc.search(text, opts, MAX_FIND_MATCHES);
        }
    }

    fn find_next(&self) {
        if let Some(fc) = self.view.find_controller() {
            fc.search_next();
        }
    }

    fn find_previous(&self) {
        // Best-effort backward step. WebKit's search_previous cannot reach
        // hidden/collapsed content and corrupts the find controller's heap
        // (surfacing as a harmless error at process exit); kept because forward
        // wraparound alone makes reverse navigation tedious. See trait docs.
        if let Some(fc) = self.view.find_controller() {
            fc.search_previous();
        }
    }

    fn find_clear(&self) {
        if let Some(fc) = self.view.find_controller() {
            fc.search_finish();
        }
    }

    fn widget(&self) -> gtk4::Widget {
        self.view.clone().upcast::<gtk4::Widget>()
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_download_path, sanitize_download_name};

    #[test]
    fn sanitize_reduces_to_safe_component() {
        assert_eq!(sanitize_download_name("a.txt"), "a.txt");
        assert_eq!(sanitize_download_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_download_name("/abs/dir/file.bin"), "file.bin");
        assert_eq!(sanitize_download_name(""), "download");
        assert_eq!(sanitize_download_name(".."), "download");
        // A URL fallback must drop its query string and fragment.
        assert_eq!(
            sanitize_download_name("https://h.test/p/file.bin?x=1&y=2"),
            "file.bin"
        );
        assert_eq!(sanitize_download_name("/p/name#frag"), "name");
    }

    #[test]
    fn safe_path_dedups_collisions() {
        let dir = std::env::temp_dir().join(format!("qbrsh-dl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let first = safe_download_path(&dir, "x.txt");
        assert_eq!(first.file_name().unwrap().to_str().unwrap(), "x.txt");
        std::fs::write(&first, b"a").unwrap();
        let second = safe_download_path(&dir, "x.txt");
        assert_ne!(first, second);
        assert_eq!(second.file_name().unwrap().to_str().unwrap(), "x-1.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
