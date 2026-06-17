//! qbrsh: a fast, keyboard-driven web browser built on a hand-rolled TEA core.
//!
//! This entry point currently runs a headless smoke check of the core message
//! loop. The GTK4/WebKitGTK window and the glib-driven dispatch loop are wired in
//! with the window/engine subsystem.

mod core;

use crate::core::command::{Command, OpenTarget};
use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::runtime::{EffectRunner, Mailbox, Runtime};
use crate::core::state::{Config, State};

/// Prints effects to stdout. Placeholder runner until the engine layer lands.
struct PrintRunner;

impl EffectRunner for PrintRunner {
    fn run(&mut self, effect: Effect, _mailbox: &Mailbox) {
        println!("effect: {effect:?}");
    }
}

fn main() {
    let mut state = State::new(Config::default());
    let homepage = state.config.homepage.clone();
    state.tabs.open(&homepage);
    state.tabs.focus_last();

    let mut runtime = Runtime::new(state, PrintRunner);
    let mailbox = runtime.mailbox();

    mailbox.send(Msg::Command(Command::Open {
        target: OpenTarget::Current,
        input: "rust-lang.org".to_string(),
    }));
    runtime.pump();

    println!(
        "core smoke check ok: {} tab(s), running={}",
        runtime.state().tabs.len(),
        runtime.running()
    );
}
