//! qbrsh: a fast, keyboard-driven web browser built on a hand-rolled TEA core.

mod adblock;
mod app;
mod config;
mod core;
mod engine;
mod history;
mod input;
mod marks;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;

const APP_ID: &str = "org.qbrsh.browser";

fn main() {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(app::run);
    app.run();
}
