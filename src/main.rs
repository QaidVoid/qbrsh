//! qbrsh: a fast, keyboard-driven web browser built on a hand-rolled TEA core.

mod adblock;
mod app;
mod config;
mod core;
mod engine;
mod history;
mod input;
mod marks;
mod plugin;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;

const APP_ID: &str = "org.qbrsh.browser";

fn main() {
    // Parse one optional URL argument ourselves, then start GTK with no args so
    // it routes to `activate` (not `open`) and ignores our argv.
    let url = std::env::args().nth(1).filter(|a| !a.starts_with('-'));
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |a| app::run(a, url.clone()));
    app.run_with_args::<&str>(&[]);
}
