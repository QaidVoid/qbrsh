//! Application assembly: builds the window and the glib-driven dispatch loop.
//!
//! The consumer task owns `State` exclusively and drains the mailbox on the glib
//! main context. GTK and WebKit signals only enqueue messages; effects are
//! carried out by [`GtkEffectRunner`], which holds the UI and the per-tab views.

use std::collections::HashMap;

use gtk4::Application;
use gtk4::prelude::*;

use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::runtime::{EffectRunner, Mailbox, dispatch};
use crate::core::state::{Config, Mode, State, TabId};
use crate::engine::traits::EngineView;
use crate::engine::webkit::WebKitEngine;
use crate::input;
use crate::ui::window::Ui;

/// Executes effects against the GTK UI and the WebKit views.
struct GtkEffectRunner {
    app: Application,
    ui: Ui,
    engine: WebKitEngine,
    views: HashMap<TabId, Box<dyn EngineView>>,
    mailbox: Mailbox,
}

impl GtkEffectRunner {
    fn render_status(&self, state: &State) {
        let mode = match state.mode.current {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
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
        let pending = format!(
            "{}{}",
            state.input.count,
            crate::core::key::display_sequence(&state.input.pending)
        );
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
            Effect::RecordHistory { .. } => {
                // History persistence is wired in the storage subsystem (group 5).
            }
            Effect::RenderStatus => self.render_status(state),
            Effect::RenderTabs => self.render_tabs(state),
            Effect::ShowMessage { text, .. } => {
                self.ui.statusbar.set_text(&text);
            }
            Effect::Quit => self.app.quit(),
        }
    }
}

/// Build the window, seed the first tab, and start the dispatch loop.
pub fn run(app: &Application) {
    let (mailbox, rx) = Mailbox::channel();
    let ui = Ui::build(app);
    let engine = WebKitEngine::new(false);

    let mut state = State::new(Config::default());
    let home = state.config.homepage.clone();
    let id = state.tabs.open(&home);
    state.tabs.focus_last();

    let mut views: HashMap<TabId, Box<dyn EngineView>> = HashMap::new();
    let view = engine.create_view(id, mailbox.clone());
    ui.stack.add_child(&view.widget());
    ui.stack.set_visible_child(&view.widget());
    view.load_uri(&home);
    views.insert(id, view);

    let mut runner = GtkEffectRunner {
        app: app.clone(),
        ui: ui.clone(),
        engine,
        views,
        mailbox: mailbox.clone(),
    };
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
