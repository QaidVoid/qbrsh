//! qbrsh: a fast, keyboard-driven web browser built on a hand-rolled TEA core.

mod adblock;
mod app;
mod config;
mod core;
mod engine;
mod history;
mod input;
mod ipc;
mod marks;
mod plugin;
mod ui;

use gtk4::Application;
use gtk4::gio::ApplicationFlags;
use gtk4::prelude::*;

const APP_ID: &str = "org.qbrsh.browser";

fn main() {
    // Parse one optional URL argument ourselves, then start GTK with no args so
    // it routes to `activate` (not `open`) and ignores our argv.
    let url = std::env::args().nth(1).filter(|a| !a.starts_with('-'));

    // If a URL was given and another instance is running, hand it off and exit.
    if let Some(ref u) = url
        && ipc::forward_url(u)
    {
        return;
    }

    // NON_UNIQUE: each launch is its own process. We do not use GApplication's
    // single-instance activation; cross-instance "open URL in running browser"
    // is handled by our own IPC (forward_url above).
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |a| app::run(a, url.clone()));
    app.run_with_args::<&str>(&[]);
}
