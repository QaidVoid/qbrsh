//! Application assembly: builds the window and the glib-driven dispatch loop.
//!
//! The consumer task owns `State` exclusively and drains the mailbox on the glib
//! main context. GTK and WebKit signals only enqueue messages; effects are
//! carried out by [`GtkEffectRunner`], which holds the UI and the per-tab views.

use std::collections::HashMap;
use std::path::PathBuf;

use gtk4::Application;
use gtk4::prelude::*;

use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::runtime::{EffectRunner, Mailbox, dispatch};
use crate::config;
use crate::core::state::{Bookmark, Mode, State, TabId};
use crate::engine::traits::EngineView;
use crate::engine::webkit::WebKitEngine;
use crate::history::History;
use crate::input;
use crate::marks;
use crate::ui::window::Ui;

/// Executes effects against the GTK UI and the WebKit views.
struct GtkEffectRunner {
    app: Application,
    ui: Ui,
    engine: WebKitEngine,
    views: HashMap<TabId, Box<dyn EngineView>>,
    history: History,
    css: gtk4::CssProvider,
    quickmarks_path: PathBuf,
    bookmarks_path: PathBuf,
    sessions_dir: PathBuf,
    mailbox: Mailbox,
}

impl GtkEffectRunner {
    fn render_status(&self, state: &State) {
        let mode = match state.mode.current {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Hint => "HINT",
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
                let view = self.engine.create_view(id, self.mailbox.clone());
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
            Effect::ReloadConfig => mailbox.send(Msg::ConfigLoaded(config::load())),
            Effect::SaveSession { name, urls } => {
                let _ = std::fs::create_dir_all(&self.sessions_dir);
                let _ = std::fs::write(self.sessions_dir.join(&name), urls.join("\n"));
            }
            Effect::LoadSession { name } => {
                let urls = std::fs::read_to_string(self.sessions_dir.join(&name))
                    .map(|s| s.lines().map(str::to_string).collect::<Vec<_>>())
                    .unwrap_or_default();
                mailbox.send(Msg::SessionLoaded(urls));
            }
            Effect::RecordHistory { uri, title } => {
                self.history.record(&uri, &title);
            }
            Effect::RenderStatus => self.render_status(state),
            Effect::RenderTabs => self.render_tabs(state),
            Effect::RenderCompletion => self.render_completion(state),
            Effect::ShowMessage { text, .. } => {
                self.ui.statusbar.set_text(&text);
            }
            Effect::Quit => self.app.quit(),
        }
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

/// Build the window, seed the first tab, and start the dispatch loop.
pub fn run(app: &Application) {
    let (mailbox, rx) = Mailbox::channel();
    let ui = Ui::build(app);
    let engine = WebKitEngine::new(false);

    let dir = data_dir();
    let quickmarks_path = dir.join("quickmarks");
    let bookmarks_path = dir.join("bookmarks");

    let mut state = State::new(config::load());
    state.quickmarks = marks::load_quickmarks(&quickmarks_path)
        .into_iter()
        .collect();
    state.bookmarks = marks::load_bookmarks(&bookmarks_path)
        .into_iter()
        .map(|(url, title)| Bookmark { url, title })
        .collect();

    let home = state.config.homepage.clone();
    let id = state.tabs.open(&home);
    state.tabs.focus_last();

    let mut views: HashMap<TabId, Box<dyn EngineView>> = HashMap::new();
    let view = engine.create_view(id, mailbox.clone());
    ui.stack.add_child(&view.widget());
    ui.stack.set_visible_child(&view.widget());
    view.load_uri(&home);
    views.insert(id, view);

    let history = History::open(&dir.join("history.db"), mailbox.clone());

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
        css,
        quickmarks_path,
        bookmarks_path,
        sessions_dir: dir.join("sessions"),
        mailbox: mailbox.clone(),
    };
    runner.apply_theme(&state);
    runner.render_status(&state);
    runner.render_tabs(&state);

    let mode_mirror = input::install(&ui, &mailbox);
    ui.window.present();

    glib::MainContext::default().spawn_local(async move {
        while let Ok(msg) = rx.recv().await {
            dispatch(&mut state, &mut runner, &mailbox, msg);
            mode_mirror.set(state.mode.current);
        }
    });
}
