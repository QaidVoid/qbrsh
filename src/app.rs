//! Application assembly: builds the window and the glib-driven dispatch loop.
//!
//! The consumer task owns `State` exclusively and drains the mailbox on the glib
//! main context. GTK and WebKit signals only enqueue messages; effects are
//! carried out by [`GtkEffectRunner`], which holds the UI and the per-tab views.

use std::collections::HashMap;
use std::path::PathBuf;

use gtk4::Application;
use gtk4::prelude::*;

use crate::core::command::{Command, OpenTarget};
use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::runtime::{EffectRunner, Mailbox, dispatch};
use crate::config;
use crate::core::state::{Bookmark, Mode, State, TabId};
use crate::engine::traits::EngineView;
use crate::engine::webkit::{PermissionMirror, WebKitEngine};
use crate::history::History;
use crate::input;
use crate::marks;
use crate::plugin::PluginRuntime;
use crate::ui::window::Ui;

/// Executes effects against the GTK UI and the WebKit views.
struct GtkEffectRunner {
    app: Application,
    ui: Ui,
    engine: WebKitEngine,
    views: HashMap<TabId, Box<dyn EngineView>>,
    history: History,
    plugins: PluginRuntime,
    permissions: PermissionMirror,
    css: gtk4::CssProvider,
    quickmarks_path: PathBuf,
    bookmarks_path: PathBuf,
    sessions_dir: PathBuf,
    permissions_path: PathBuf,
    mailbox: Mailbox,
}

impl GtkEffectRunner {
    /// Resolve a session file path, rejecting names that are not a single safe
    /// path component so a session save or load cannot escape the sessions
    /// directory.
    fn session_path(&self, name: &str) -> Option<PathBuf> {
        let name = name.trim();
        if name.is_empty()
            || name == ".."
            || name == "."
            || name.contains(['/', '\\'])
        {
            return None;
        }
        Some(self.sessions_dir.join(name))
    }

    fn render_status(&self, state: &State) {
        // A pending permission prompt takes over the status bar.
        if state.mode.current == Mode::Prompt
            && let Some(p) = state.prompts.front()
        {
            self.ui.statusbar.set_text(&format!(
                "Allow {} for {}?  [y]es  [n]o  [a]lways  [d]eny-always  (Esc denies)",
                p.capability.as_str(),
                p.host,
            ));
            self.ui.commandline.set_visible(false);
            return;
        }
        let mode = match state.mode.current {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Hint => "HINT",
            Mode::Prompt => "PROMPT",
            Mode::Permissions => "PERMISSIONS",
        };
        let url = state.tabs.active().map(|t| t.url.clone()).unwrap_or_default();
        let prog = state.tabs.active().map(|t| t.progress).unwrap_or(0.0);
        let progress = if prog > 0.0 && prog < 1.0 {
            format!(" ({}%)", (prog * 100.0) as u32)
        } else {
            String::new()
        };
        let scroll = state
            .status
            .scroll_percent
            .map(|p| format!("  {p}%"))
            .unwrap_or_default();
        let pending = if state.mode.current == Mode::Hint {
            state.hints.input.clone()
        } else {
            format!(
                "{}{}",
                state.input.count,
                crate::core::key::display_sequence(&state.input.pending)
            )
        };
        let pending = if pending.is_empty() {
            String::new()
        } else {
            format!("  {pending}")
        };
        self.ui
            .statusbar
            .set_text(&format!("-- {mode} --  {url}{progress}{scroll}{pending}"));

        if state.command_line.active {
            if self.ui.commandline.text().as_str() != state.command_line.text.as_str() {
                self.ui.commandline.set_text(&state.command_line.text);
            }
            self.ui.commandline.set_visible(true);
            self.ui.commandline.grab_focus();
            self.ui.commandline.set_position(-1);
        } else {
            self.ui.commandline.set_visible(false);
        }
    }

    fn apply_theme(&self, state: &State) {
        let c = &state.config;
        let css = format!(
            "#qbrsh-tabbar, #qbrsh-status, #qbrsh-cmd, #qbrsh-completion {{ \
                background-color: {bg}; color: {fg}; \
                font-family: {ff}; font-size: {fs}px; }}\n\
             #qbrsh-tabbar, #qbrsh-status {{ padding: 2px 6px; }}\n\
             #qbrsh-completion label {{ padding: 1px 6px; }}",
            bg = c.colors.background,
            fg = c.colors.foreground,
            ff = c.font.family,
            fs = c.font.size,
        );
        self.css.load_from_data(&css);
    }

    fn render_completion(&self, state: &State) {
        while let Some(child) = self.ui.completion.first_child() {
            self.ui.completion.remove(&child);
        }
        let show = state.mode.current == Mode::Command && !state.completion.items.is_empty();
        self.ui.completion.set_visible(show);
        if !show {
            return;
        }
        for (i, item) in state.completion.items.iter().enumerate() {
            let marker = if Some(i) == state.completion.selected {
                "▸ "
            } else {
                "  "
            };
            let label = gtk4::Label::new(Some(&format!("{marker}{}", item.display)));
            label.set_xalign(0.0);
            self.ui.completion.append(&label);
        }
    }

    fn render_permissions(&self, state: &State) {
        while let Some(child) = self.ui.completion.first_child() {
            self.ui.completion.remove(&child);
        }
        let show = state.mode.current == Mode::Permissions;
        self.ui.completion.set_visible(show);
        if !show {
            return;
        }
        if state.perm_view.rows.is_empty() {
            let label = gtk4::Label::new(Some("no per-site permissions set"));
            label.set_xalign(0.0);
            self.ui.completion.append(&label);
            return;
        }
        let header = gtk4::Label::new(Some(
            "  [a]llow  [d]eny  a[s]k  [x] revoke  j/k move  Esc close",
        ));
        header.set_xalign(0.0);
        self.ui.completion.append(&header);
        for (i, row) in state.perm_view.rows.iter().enumerate() {
            let marker = if i == state.perm_view.selected {
                "▸ "
            } else {
                "  "
            };
            let cap = row.capability.map(|c| c.as_str()).unwrap_or("all");
            let policy = match row.policy {
                crate::core::state::PermissionPolicy::Allow => "allow",
                crate::core::state::PermissionPolicy::Ask => "ask",
                crate::core::state::PermissionPolicy::Deny => "deny",
            };
            let label =
                gtk4::Label::new(Some(&format!("{marker}{}  {cap}  {policy}", row.host)));
            label.set_xalign(0.0);
            self.ui.completion.append(&label);
        }
    }

    fn render_tabs(&self, state: &State) {
        let n = state.tabs.len();
        let idx = state.tabs.active_index() + 1;
        let title = state
            .tabs
            .active()
            .map(|t| {
                if t.title.is_empty() {
                    t.url.clone()
                } else {
                    t.title.clone()
                }
            })
            .unwrap_or_default();
        self.ui.tabbar.set_text(&format!("[{idx}/{n}] {title}"));
        let win_title = if title.is_empty() { "qbrsh" } else { &title };
        self.ui.window.set_title(Some(&format!("{win_title} - qbrsh")));
    }
}

impl EffectRunner for GtkEffectRunner {
    fn run(&mut self, effect: Effect, state: &State, mailbox: &Mailbox) {
        match effect {
            Effect::LoadUri { tab, uri } => {
                if let Some(v) = self.views.get(&tab) {
                    v.load_uri(&uri);
                }
            }
            Effect::Reload { tab, bypass_cache } => {
                if let Some(v) = self.views.get(&tab) {
                    v.reload(bypass_cache);
                }
            }
            Effect::Stop { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    v.stop();
                }
            }
            Effect::GoBack { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    v.go_back();
                }
            }
            Effect::GoForward { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    v.go_forward();
                }
            }
            Effect::EvalJs {
                id, tab, script, ..
            } => {
                if let Some(v) = self.views.get(&tab) {
                    let mb = mailbox.clone();
                    v.evaluate_js(
                        &script,
                        Box::new(move |result| {
                            mb.send(Msg::JsResult { id, tab, result });
                        }),
                    );
                }
            }
            Effect::OpenTab {
                id,
                uri,
                background,
            } => {
                let view = self.engine.create_view(id, &uri, self.mailbox.clone());
                self.ui.stack.add_child(&view.widget());
                if !background {
                    self.ui.stack.set_visible_child(&view.widget());
                }
                view.load_uri(&uri);
                self.views.insert(id, view);
            }
            Effect::CloseTab { tab } => {
                if let Some(v) = self.views.remove(&tab) {
                    self.ui.stack.remove(&v.widget());
                }
            }
            Effect::FocusTab { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    self.ui.stack.set_visible_child(&v.widget());
                }
            }
            Effect::SetClipboard(text) => {
                self.ui.window.clipboard().set_text(&text);
            }
            Effect::SaveQuickmarks(entries) => {
                marks::save_quickmarks(&self.quickmarks_path, &entries);
            }
            Effect::SaveBookmarks(entries) => {
                marks::save_bookmarks(&self.bookmarks_path, &entries);
            }
            Effect::QueryHistory {
                query,
                prefix,
                generation,
            } => {
                self.history.query(query, prefix, generation);
            }
            Effect::ApplyTheme => self.apply_theme(state),
            Effect::SyncPermissions(permissions) => *self.permissions.borrow_mut() = permissions,
            Effect::ResolvePermission { id, allow } => self.engine.resolve_permission(id, allow),
            Effect::SavePermissions(permissions) => {
                config::save_permissions(&self.permissions_path, &permissions);
            }
            Effect::ReloadConfig => mailbox.send(Msg::ConfigLoaded(config::load())),
            Effect::SaveSession { name, urls } => {
                if let Some(path) = self.session_path(&name) {
                    let _ = std::fs::create_dir_all(&self.sessions_dir);
                    let _ = std::fs::write(path, urls.join("\n"));
                }
            }
            Effect::LoadSession { name } => {
                let urls = self
                    .session_path(&name)
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .map(|s| s.lines().map(str::to_string).collect::<Vec<_>>())
                    .unwrap_or_default();
                mailbox.send(Msg::SessionLoaded(urls));
            }
            Effect::FireHook { event, arg } => self.plugins.fire(&event, &arg),
            Effect::ReloadPlugins => {
                self.plugins.reload();
                self.ui.statusbar.set_text(&format!(
                    "reloaded {} plugin(s)",
                    self.plugins.count()
                ));
            }
            Effect::PluginEval { id, tab, script } => {
                if let Some(v) = self.views.get(&tab) {
                    let mb = mailbox.clone();
                    v.evaluate_js(
                        &script,
                        Box::new(move |result| {
                            mb.send(Msg::PluginEvalResult {
                                id,
                                result: result.unwrap_or_default(),
                            });
                        }),
                    );
                } else {
                    mailbox.send(Msg::PluginEvalResult {
                        id,
                        result: String::new(),
                    });
                }
            }
            Effect::ResolvePluginEval { id, result } => self.plugins.resolve(id, result),
            Effect::RecordHistory { uri, title } => {
                self.history.record(&uri, &title);
            }
            Effect::RenderStatus => self.render_status(state),
            Effect::RenderTabs => self.render_tabs(state),
            Effect::RenderCompletion => self.render_completion(state),
            Effect::RenderPermissions => self.render_permissions(state),
            Effect::ShowMessage { text, .. } => {
                self.ui.statusbar.set_text(&text);
            }
            Effect::ReportMemory => {
                self.ui.statusbar.set_text(&memory_report(self.views.len()));
            }
            Effect::Quit => self.app.quit(),
        }
    }
}

/// Resident memory of this process in bytes, read from `/proc/self/statm`
/// (Linux). The second field is resident pages; the page size is the usual 4 KiB.
fn resident_memory_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096)
}

/// A one-line resource report: resident memory and the number of live views.
fn memory_report(views: usize) -> String {
    match resident_memory_bytes() {
        Some(bytes) => format!("memory {} MB  views {views}", bytes / 1_048_576),
        None => format!("memory unavailable  views {views}"),
    }
}

/// The XDG data directory for qbrsh, created if missing.
fn data_dir() -> PathBuf {
    let dir = directories::ProjectDirs::from("", "", "qbrsh")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Build the window, seed the first tab, and start the dispatch loop. Opens
/// `initial_url` if given, otherwise the configured homepage.
pub fn run(app: &Application, initial_url: Option<String>) {
    let (mailbox, rx) = Mailbox::channel();
    let ui = Ui::build(app);
    let dir = data_dir();
    let mut config = config::load();
    // Layer the runtime permission store (prompt/management grants) over the
    // user-authored config defaults; the store wins per site.
    let permissions_path = dir.join("permissions.toml");
    if let Some(stored) = config::load_permissions(&permissions_path) {
        for (host, rules) in stored.sites {
            config.permissions.sites.insert(host, rules);
        }
    }
    let blocklist = std::rc::Rc::new(crate::adblock::load(&dir.join("adblock")));
    let permissions: PermissionMirror =
        std::rc::Rc::new(std::cell::RefCell::new(config.permissions.clone()));
    let engine = WebKitEngine::new(
        false,
        blocklist,
        permissions.clone(),
        &dir.join("content-filters"),
    );

    let quickmarks_path = dir.join("quickmarks");
    let bookmarks_path = dir.join("bookmarks");

    let mut state = State::new(config);
    state.quickmarks = marks::load_quickmarks(&quickmarks_path)
        .into_iter()
        .collect();
    state.bookmarks = marks::load_bookmarks(&bookmarks_path)
        .into_iter()
        .map(|(url, title)| Bookmark { url, title })
        .collect();

    let home = initial_url.unwrap_or_else(|| state.config.homepage.clone());
    let id = state.tabs.open(&home);
    state.tabs.focus_last();

    let mut views: HashMap<TabId, Box<dyn EngineView>> = HashMap::new();
    let view = engine.create_view(id, &home, mailbox.clone());
    ui.stack.add_child(&view.widget());
    ui.stack.set_visible_child(&view.widget());
    views.insert(id, view);
    // Load the initial page through the normalizing Open path (so a bare host
    // like `github.com` gets a scheme).
    mailbox.send(Msg::Command(Command::Open {
        target: OpenTarget::Current,
        input: home,
    }));

    let history = History::open(&dir.join("history.db"), mailbox.clone());
    let plugins = PluginRuntime::new(dir.join("plugins"), mailbox.clone());

    let css = gtk4::CssProvider::new();
    if let Some(display) = gdk4::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let mut runner = GtkEffectRunner {
        app: app.clone(),
        ui: ui.clone(),
        engine,
        views,
        history,
        plugins,
        permissions,
        css,
        quickmarks_path,
        bookmarks_path,
        sessions_dir: dir.join("sessions"),
        permissions_path,
        mailbox: mailbox.clone(),
    };
    runner.apply_theme(&state);
    runner.render_status(&state);
    runner.render_tabs(&state);
    eprintln!("[qbrsh] startup {}", memory_report(runner.views.len()));

    let mode_mirror = input::install(&ui, &mailbox);
    crate::ipc::serve(mailbox.clone());
    ui.window.present();

    glib::MainContext::default().spawn_local(async move {
        while let Ok(msg) = rx.recv().await {
            dispatch(&mut state, &mut runner, &mailbox, msg);
            mode_mirror.set(input::UiView {
                mode: state.mode.current,
                completion_active: state.completion.selected.is_some(),
            });
        }
    });
}
