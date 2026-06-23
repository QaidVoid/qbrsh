//! Application assembly: builds the window and the glib-driven dispatch loop.
//!
//! The consumer task owns `State` exclusively and drains the mailbox on the glib
//! main context. GTK and WebKit signals only enqueue messages; effects are
//! carried out by [`GtkEffectRunner`], which holds the UI and the per-tab views.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::Application;
use gtk4::prelude::*;

use crate::config;
use crate::core::command::Command;
use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::runtime::{EffectRunner, Mailbox, dispatch};
use crate::core::state::{Bookmark, Layout, LayoutNode, Mode, SplitDir, State, TabId};
use crate::engine::traits::EngineView;
use crate::engine::webkit::{FaviconStore, PermissionMirror, SitePrefsMirror, WebKitEngine};
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
    /// A persistent wrapper widget per tab, holding its view. The layout
    /// reparents these wrappers into a `GtkPaned` tree; the view inside is never
    /// destroyed across rebuilds.
    pane_wrappers: HashMap<TabId, gtk4::Box>,
    history: History,
    plugins: PluginRuntime,
    permissions: PermissionMirror,
    site_prefs: SitePrefsMirror,
    favicons: FaviconStore,
    css: gtk4::CssProvider,
    quickmarks_path: PathBuf,
    bookmarks_path: PathBuf,
    sessions_dir: PathBuf,
    autosave_path: PathBuf,
    permissions_path: PathBuf,
    site_prefs_path: PathBuf,
    mailbox: Mailbox,
}

impl GtkEffectRunner {
    /// Resolve a session file path, rejecting names that are not a single safe
    /// path component so a session save or load cannot escape the sessions
    /// directory.
    fn session_path(&self, name: &str) -> Option<PathBuf> {
        let name = name.trim();
        if name.is_empty() || name == ".." || name == "." || name.contains(['/', '\\']) {
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
            Mode::Downloads => "DOWNLOADS",
            Mode::History => "HISTORY",
        };
        let url = state
            .tabs
            .active()
            .map(|t| t.url.clone())
            .unwrap_or_default();
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
        let search = state
            .status
            .search
            .as_ref()
            .map(|s| format!("  [{}]", s.label()))
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
        let private = if state.tabs.active().is_some_and(|t| t.private) {
            "  [private]"
        } else {
            ""
        };
        self.ui.statusbar.set_text(&format!(
            "-- {mode} --{private}  {url}{progress}{scroll}{search}{pending}"
        ));

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
            "#qbrsh-tabs, #qbrsh-tabs-scroll, #qbrsh-status, #qbrsh-cmd, #qbrsh-completion {{ \
                background-color: {bg}; color: {fg}; \
                font-family: {ff}; font-size: {fs}px; }}\n\
             #qbrsh-status {{ padding: 2px 6px; }}\n\
             #qbrsh-completion label {{ padding: 1px 6px; }}\n\
             #qbrsh-tabs .tab-row {{ padding: 3px 8px; }}\n\
             #qbrsh-tabs .tab-collapsed {{ padding: 3px 0; }}\n\
             #qbrsh-tabs .tab-mounted {{ border-left: 2px solid {accent}; }}\n\
             #qbrsh-tabs .tab-private {{ border-right: 3px solid {accent}; font-style: italic; }}\n\
             #qbrsh-tabs .tab-active {{ background-color: {accent}; }}\n\
             #qbrsh-tabs .tab-active label {{ color: {bg}; }}\n\
             #qbrsh-layout {{ background-color: {bg}; }}\n\
             .pane {{ padding: 0; }}\n\
             .pane-focused {{ outline: 2px solid {accent}; outline-offset: -2px; }}",
            bg = c.colors.background,
            fg = c.colors.foreground,
            ff = c.font.family,
            fs = c.font.size,
            accent = c.colors.accent,
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
            let label = gtk4::Label::new(Some(&format!("{marker}{}  {cap}  {policy}", row.host)));
            label.set_xalign(0.0);
            self.ui.completion.append(&label);
        }
    }

    fn render_downloads(&self, state: &State) {
        while let Some(child) = self.ui.completion.first_child() {
            self.ui.completion.remove(&child);
        }
        let show = state.mode.current == Mode::Downloads;
        self.ui.completion.set_visible(show);
        if !show {
            return;
        }
        if state.downloads.is_empty() {
            let label = gtk4::Label::new(Some("no downloads yet"));
            label.set_xalign(0.0);
            self.ui.completion.append(&label);
            return;
        }
        let header = gtk4::Label::new(Some(
            "  [o] open  [r] reveal  [c] cancel  [R] retry  [x] clear  j/k move  Esc close",
        ));
        header.set_xalign(0.0);
        self.ui.completion.append(&header);
        // Newest first: iterate download ids in descending order.
        let ordered: Vec<_> = state.downloads.iter().rev().collect();
        let selected = state.dl_view.selected.min(ordered.len().saturating_sub(1));
        for (i, (id, dl)) in ordered.iter().enumerate() {
            let marker = if i == selected { "▸ " } else { "  " };
            let status = dl.status.as_str();
            let progress = match dl.status {
                crate::core::state::DownloadStatus::Finished => "done".to_string(),
                _ => match dl.fraction() {
                    Some(f) => format!(
                        "{}/{} ({:.0}%)",
                        fmt_bytes(dl.received),
                        fmt_bytes(dl.total),
                        f * 100.0
                    ),
                    None => fmt_bytes(dl.received),
                },
            };
            let label = gtk4::Label::new(Some(&format!(
                "{marker}#{id}  {}  [{status}]  {progress}",
                dl.filename
            )));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(120);
            self.ui.completion.append(&label);
        }
    }

    fn render_history(&self, state: &State) {
        while let Some(child) = self.ui.completion.first_child() {
            self.ui.completion.remove(&child);
        }
        let show = state.mode.current == Mode::History;
        self.ui.completion.set_visible(show);
        if !show {
            return;
        }
        // Filter line: shows the active query and whether it is being edited.
        let filter_line = if state.history_view.filter.is_empty() {
            "/ to filter".to_string()
        } else {
            format!("filter: {}", state.history_view.filter)
        };
        let edit_marker = if state.history_view.filter_edit {
            " (editing)"
        } else {
            ""
        };
        let filter_label = gtk4::Label::new(Some(&format!("  {filter_line}{edit_marker}")));
        filter_label.set_xalign(0.0);
        self.ui.completion.append(&filter_label);

        if state.history_view.rows.is_empty() {
            let label = gtk4::Label::new(Some("  no history matches"));
            label.set_xalign(0.0);
            self.ui.completion.append(&label);
            return;
        }
        let header = gtk4::Label::new(Some(
            "  [Enter/o] open  [t] new tab  [x] delete  j/k move  / filter  Esc close",
        ));
        header.set_xalign(0.0);
        self.ui.completion.append(&header);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let selected = state
            .history_view
            .selected
            .min(state.history_view.rows.len() - 1);
        for (i, row) in state.history_view.rows.iter().enumerate() {
            let marker = if i == selected { "▸ " } else { "  " };
            let title = if row.title.is_empty() {
                row.url.as_str()
            } else {
                row.title.as_str()
            };
            let label = gtk4::Label::new(Some(&format!(
                "{marker}{title}  {}  [{}x]  ({})",
                row.url,
                row.visit_count,
                fmt_ago(row.last_visit, now)
            )));
            label.set_xalign(0.0);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(120);
            self.ui.completion.append(&label);
        }
    }

    /// Rebuild the pane layout (`GtkPaned` tree) from `state.layout`. The view
    /// widgets are kept alive in `pane_wrappers`; this reparents them.
    fn render_layout(&self, state: &State) {
        while let Some(child) = self.ui.layout_area.first_child() {
            self.ui.layout_area.remove(&child);
        }
        if state.layout.is_empty() {
            return;
        }
        if let Some(widget) = self.build_node(&state.layout.root, state) {
            self.ui.layout_area.append(&widget);
        }
        self.apply_focus(state);
    }

    /// Recursively build the widget tree for a layout node.
    fn build_node(&self, node: &LayoutNode, state: &State) -> Option<gtk4::Widget> {
        match node {
            LayoutNode::Leaf(pane_id) => {
                let pane = state.layout.panes.iter().find(|p| &p.id == pane_id)?;
                let wrapper = self.pane_wrappers.get(&pane.tab)?.clone();
                // Detach from a previous parent so the new tree can own it.
                let _ = wrapper.parent();
                wrapper.unparent();
                Some(wrapper.upcast())
            }
            LayoutNode::Split { dir, a, b, .. } => {
                // `:split` (Horizontal) stacks panes; `:vsplit` (Vertical) is
                // side by side. Map to GtkPaned orientation in one place.
                let orientation = match dir {
                    SplitDir::Horizontal => gtk4::Orientation::Vertical,
                    SplitDir::Vertical => gtk4::Orientation::Horizontal,
                };
                let paned = gtk4::Paned::new(orientation);
                paned.set_vexpand(true);
                paned.set_hexpand(true);
                paned.set_wide_handle(true);
                if let Some(start) = self.build_node(a, state) {
                    paned.set_start_child(Some(&start));
                }
                if let Some(end) = self.build_node(b, state) {
                    paned.set_end_child(Some(&end));
                }
                // All splits are created at 0.5; GtkPaned defaults to the
                // midpoint, so no explicit position is set (drag drags the
                // handle natively; the stored ratio is reserved for future use).
                Some(paned.upcast())
            }
        }
    }

    /// Apply the `pane-focused` class to the focused pane and grab focus, without
    /// rebuilding the tree. Runs on focus changes and after `render_layout`.
    fn apply_focus(&self, state: &State) {
        let focused_tab = state.layout.focused_pane().map(|p| p.tab);
        for (tab, wrapper) in &self.pane_wrappers {
            if Some(*tab) == focused_tab {
                wrapper.add_css_class("pane-focused");
                if let Some(v) = self.views.get(tab) {
                    v.widget().grab_focus();
                }
            } else {
                wrapper.remove_css_class("pane-focused");
            }
        }
    }

    fn render_tabs(&self, state: &State) {
        // Rebuild the row list; clearing drops each row's click gesture.
        while let Some(child) = self.ui.tab_list.first_child() {
            self.ui.tab_list.remove(&child);
        }
        let active = state.tabs.active_id();
        let split = state.layout.panes.len() > 1;
        let collapsed = state.tabs_collapsed;
        let favicons = self.favicons.borrow();
        for (i, tab) in state.tabs.iter().enumerate() {
            let text = if tab.title.is_empty() {
                tab.url.as_str()
            } else {
                tab.title.as_str()
            };
            // Each row is a horizontal box: a favicon (or placeholder) and, when
            // expanded, the title. The gesture and state classes live on the row.
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            row.add_css_class("tab-row");
            if Some(tab.id) == active {
                row.add_css_class("tab-active");
            }
            if split && state.layout.pane_with_tab(tab.id).is_some() {
                row.add_css_class("tab-mounted");
            }
            if tab.private {
                row.add_css_class("tab-private");
            }

            let icon = match favicons.get(&tab.id) {
                Some(texture) => gtk4::Image::from_paintable(Some(texture)),
                None => gtk4::Image::from_icon_name("text-x-generic-symbolic"),
            };
            icon.set_pixel_size(16);
            row.append(&icon);

            if collapsed {
                // Icon-only rail: center the icon, tighten padding, keep the
                // title as a tooltip.
                row.add_css_class("tab-collapsed");
                icon.set_hexpand(true);
                icon.set_halign(gtk4::Align::Center);
                row.set_tooltip_text(Some(text));
            } else {
                let label = gtk4::Label::new(Some(text));
                label.set_xalign(0.0);
                label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
                label.set_width_chars(0);
                label.set_hexpand(true);
                row.append(&label);
            }

            // Click focuses the tab through the same path as keyboard switching.
            let gesture = gtk4::GestureClick::new();
            gesture.set_button(gtk4::gdk::BUTTON_PRIMARY);
            let mailbox = self.mailbox.clone();
            let index = i + 1; // TabSelect is 1-based.
            gesture.connect_released(move |_g, _n, _x, _y| {
                mailbox.send(Msg::Command(Command::TabSelect(index)));
            });
            row.add_controller(gesture);
            self.ui.tab_list.append(&row);
        }
        drop(favicons);
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
        let win_title = if title.is_empty() { "qbrsh" } else { &title };
        self.ui
            .window
            .set_title(Some(&format!("{win_title} - qbrsh")));
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
            Effect::SetZoom { tab, level } => {
                if let Some(v) = self.views.get(&tab) {
                    v.set_zoom(level);
                }
            }
            Effect::Find { tab, text } => {
                if let Some(v) = self.views.get(&tab) {
                    v.find(&text);
                }
            }
            Effect::FindNext { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    v.find_next();
                }
            }
            Effect::FindPrev { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    v.find_previous();
                }
            }
            Effect::FindClear { tab } => {
                if let Some(v) = self.views.get(&tab) {
                    v.find_clear();
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
                background: _,
                private,
            } => {
                let view = self
                    .engine
                    .create_view(id, &uri, private, self.mailbox.clone());
                if let Some(t) = state.tabs.get(id) {
                    view.set_zoom(t.zoom);
                }
                view.load_uri(&uri);
                // Wrap the view in a persistent box the layout reparents. It is
                // NOT parented here; `render_layout` mounts the mounted panes.
                let wrapper = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                wrapper.set_widget_name("qbrsh-pane");
                wrapper.add_css_class("pane");
                wrapper.set_vexpand(true);
                wrapper.set_hexpand(true);
                wrapper.append(&view.widget());
                self.pane_wrappers.insert(id, wrapper);
                self.views.insert(id, view);
            }
            Effect::CloseTab { tab } => {
                if let Some(wrapper) = self.pane_wrappers.remove(&tab) {
                    wrapper.unparent();
                }
                self.views.remove(&tab);
                self.favicons.borrow_mut().remove(&tab);
            }
            Effect::FocusPane { pane: _ } => self.apply_focus(state),
            Effect::RenderLayout => self.render_layout(state),
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
            Effect::QueryHistoryView { query, generation } => {
                self.history.query_view(query, generation);
            }
            Effect::DeleteHistory { url } => self.history.delete(url),
            Effect::ApplyTheme => self.apply_theme(state),
            Effect::SyncPermissions(permissions) => *self.permissions.borrow_mut() = permissions,
            Effect::ResolvePermission { id, allow } => self.engine.resolve_permission(id, allow),
            Effect::SavePermissions(permissions) => {
                config::save_permissions(&self.permissions_path, &permissions);
            }
            Effect::SyncSitePrefs(prefs) => *self.site_prefs.borrow_mut() = prefs,
            Effect::SaveSitePrefs(prefs) => {
                config::save_site_prefs(&self.site_prefs_path, &prefs);
            }
            Effect::SetJavascript { tab, enabled } => {
                if let Some(view) = self.views.get(&tab) {
                    view.set_javascript_enabled(enabled);
                }
            }
            Effect::CancelDownload { id } => self.engine.cancel_download(id),
            Effect::RetryDownload { id } => {
                if let Some(dl) = state.downloads.get(&id) {
                    self.engine.retry_download(&dl.source);
                }
            }
            Effect::OpenPath { path } => open_path_external(&path),
            Effect::RevealPath { path } => {
                if let Some(parent) = path.parent() {
                    open_path_external(parent);
                } else {
                    open_path_external(&path);
                }
            }
            Effect::ClearData { scope, host } => {
                self.engine.clear_website_data(scope, host, mailbox.clone());
            }
            Effect::ReloadConfig => mailbox.send(Msg::ConfigLoaded(Box::new(config::load()))),
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
            Effect::SaveAutosave { urls, active } => {
                save_autosave(&self.autosave_path, &Autosave { active, urls });
            }
            Effect::FireHook { event, arg } => self.plugins.fire(&event, &arg),
            Effect::ReloadPlugins => {
                self.plugins.reload();
                self.ui
                    .statusbar
                    .set_text(&format!("reloaded {} plugin(s)", self.plugins.count()));
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
            Effect::SetTabWidth(width) => self.ui.split.set_position(width as i32),
            Effect::ToggleFullscreen { fullscreen } => {
                if fullscreen {
                    self.ui.window.fullscreen();
                } else {
                    self.ui.window.unfullscreen();
                }
            }
            Effect::RenderCompletion => self.render_completion(state),
            Effect::RenderPermissions => self.render_permissions(state),
            Effect::RenderDownloads => self.render_downloads(state),
            Effect::RenderHistory => self.render_history(state),
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

/// Format a byte count with a binary unit suffix (KiB, MiB, ...), 1 decimal.
fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Format a Unix timestamp as a compact relative age (e.g. "5m ago"), avoiding a
/// date-library dependency. `now` is the current Unix seconds.
fn fmt_ago(unix: i64, now: i64) -> String {
    let s = (now - unix).max(0);
    if s < 60 {
        return "just now".to_string();
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m ago");
    }
    let h = m / 60;
    if h < 24 {
        return format!("{h}h ago");
    }
    let d = h / 24;
    if d < 30 {
        return format!("{d}d ago");
    }
    let mo = d / 30;
    if mo < 12 {
        return format!("{mo}mo ago");
    }
    format!("{}y ago", d / 365)
}

/// Launch `xdg-open` on `path` (best-effort), used to open a finished download
/// or its containing folder. The browser is Linux-only already (it reads
/// `/proc/self/statm` for memory), so a desktop portal helper is sufficient.
fn open_path_external(path: &std::path::Path) {
    match std::process::Command::new("xdg-open").arg(path).spawn() {
        Ok(mut child) => {
            // Reap to avoid zombies; the launcher returns immediately.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("[qbrsh] could not open {}: {e}", path.display()),
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

/// The live session persisted on shutdown and restored on startup: the open
/// tabs' URLs and the index of the active tab.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Autosave {
    active: usize,
    urls: Vec<String>,
}

/// Load the autosave at `path`. Returns `None` when the file is missing or does
/// not parse, so startup falls back to the homepage.
fn load_autosave(path: &Path) -> Option<Autosave> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Persist the autosave to `path`, creating the parent directory if needed.
fn save_autosave(path: &Path, autosave: &Autosave) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(autosave) {
        let _ = std::fs::write(path, text);
    }
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
    // Load the per-domain site-preference store and share it with the engine.
    let site_prefs_path = dir.join("site-prefs.toml");
    let site_prefs_store = config::load_site_prefs(&site_prefs_path).unwrap_or_default();
    let site_prefs: SitePrefsMirror =
        std::rc::Rc::new(std::cell::RefCell::new(site_prefs_store.clone()));
    let favicons: FaviconStore = std::rc::Rc::new(std::cell::RefCell::new(HashMap::new()));
    let downloads_dir = directories::UserDirs::new()
        .and_then(|u| u.download_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| dir.join("downloads"));
    let _ = std::fs::create_dir_all(&downloads_dir);
    let engine = WebKitEngine::new(
        false,
        config.useragent.clone(),
        blocklist,
        permissions.clone(),
        site_prefs.clone(),
        favicons.clone(),
        &dir,
        &dir.join("content-filters"),
        &downloads_dir,
        mailbox.clone(),
    );

    let quickmarks_path = dir.join("quickmarks");
    let bookmarks_path = dir.join("bookmarks");

    let mut state = State::new(config);
    state.site_prefs = site_prefs_store;
    state.quickmarks = marks::load_quickmarks(&quickmarks_path)
        .into_iter()
        .collect();
    state.bookmarks = marks::load_bookmarks(&bookmarks_path)
        .into_iter()
        .map(|(url, title)| Bookmark { url, title })
        .collect();

    // Decide the initial tab set: restore the autosaved session on a clean
    // launch (no explicit URL) when restore is enabled, else open the homepage.
    let autosave_path = dir.join("session.json");
    let (initial_urls, active_index) = match initial_url {
        Some(url) => (vec![url], 0),
        None if state.config.session.restore => match load_autosave(&autosave_path) {
            Some(a) if !a.urls.is_empty() => {
                let active = a.active.min(a.urls.len() - 1);
                (a.urls, active)
            }
            _ => (vec![state.config.homepage.clone()], 0),
        },
        None => (vec![state.config.homepage.clone()], 0),
    };

    let default_zoom = state.config.zoom.default;
    let mut views: HashMap<TabId, Box<dyn EngineView>> = HashMap::new();
    let mut pane_wrappers: HashMap<TabId, gtk4::Box> = HashMap::new();
    let mut active_tab_id = TabId(0);
    for (i, raw) in initial_urls.iter().enumerate() {
        // Normalize (so a bare host gains a scheme) and load directly into the
        // new view; this mirrors the runtime Open path without depending on
        // which tab is active when the message is later dispatched.
        let uri = crate::core::update::normalize_target(raw, &state.config.search);
        let id = state.tabs.open(&uri);
        if i == active_index {
            active_tab_id = id;
        }
        if let Some(t) = state.tabs.get_mut(id) {
            t.zoom = default_zoom;
        }
        let view = engine.create_view(id, &uri, false, mailbox.clone());
        view.set_zoom(default_zoom);
        view.load_uri(&uri);
        // Wrap each view in a persistent box the layout reparents.
        let wrapper = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        wrapper.set_widget_name("qbrsh-pane");
        wrapper.add_css_class("pane");
        wrapper.set_vexpand(true);
        wrapper.set_hexpand(true);
        wrapper.append(&view.widget());
        pane_wrappers.insert(id, wrapper);
        views.insert(id, view);
    }
    // Focus the saved active tab and initialize a single-pane layout for it.
    state.tabs.focus_id(active_tab_id);
    state.layout = Layout::new(active_tab_id);

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
        pane_wrappers,
        history,
        plugins,
        permissions,
        site_prefs,
        favicons,
        css,
        quickmarks_path,
        bookmarks_path,
        sessions_dir: dir.join("sessions"),
        autosave_path,
        permissions_path,
        site_prefs_path,
        mailbox: mailbox.clone(),
    };
    runner.apply_theme(&state);
    runner.render_layout(&state);
    runner.render_status(&state);
    runner.render_tabs(&state);
    // Seed the sidebar width (collapsed rail or configured width); the divider
    // is draggable from here on.
    let initial_tab_width = if state.tabs_collapsed {
        crate::core::state::TABS_COLLAPSED_WIDTH
    } else {
        state.config.tabs.width
    };
    ui.split.set_position(initial_tab_width as i32);
    eprintln!("[qbrsh] startup {}", memory_report(runner.views.len()));

    let mode_mirror = input::install(&ui, &mailbox);

    // Closing the window forwards as a quit command so the same autosave-then-
    // quit path runs; the default close is inhibited until that completes.
    let mb = mailbox.clone();
    ui.window.connect_close_request(move |_| {
        mb.send(Msg::Command(Command::Quit));
        glib::Propagation::Stop
    });

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "qbrsh-autosave-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("session.json");
        let autosave = Autosave {
            active: 1,
            urls: vec!["https://a.test".to_string(), "https://b.test".to_string()],
        };
        save_autosave(&path, &autosave);
        let loaded = load_autosave(&path).expect("autosave loads");
        assert_eq!(loaded.active, 1);
        assert_eq!(loaded.urls, autosave.urls);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_autosave_missing_or_corrupt_is_none() {
        let dir = std::env::temp_dir().join(format!(
            "qbrsh-autosave-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Missing file.
        assert!(load_autosave(&dir.join("nope.json")).is_none());
        // Corrupt file.
        let corrupt = dir.join("corrupt.json");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&corrupt, "{ not json");
        assert!(load_autosave(&corrupt).is_none());
        let _ = std::fs::remove_file(&corrupt);
    }
}
