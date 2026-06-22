//! The message queue and dispatch.
//!
//! Every source of change enqueues a [`Msg`] on a [`Mailbox`]. A single consumer
//! drains the queue, applying each message through [`update`] and handing the
//! resulting effects to an [`EffectRunner`]. Mutation happens only in the drain,
//! so no two mutations overlap and there is no re-entrancy: an effect that needs
//! more work enqueues a new message rather than re-entering `update`.
//!
//! The queue is an `async-channel`. The GTK app drives [`dispatch`] from an
//! `async-channel` consumer task on the glib main context, which owns `State`
//! exclusively; the synchronous [`Runtime::pump`] drains the same channel for
//! tests and any non-async driver.

use async_channel::{Receiver, Sender};

use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::state::State;
use crate::core::update::update;

/// A cloneable handle for enqueuing messages onto the queue.
#[derive(Clone)]
pub struct Mailbox {
    tx: Sender<Msg>,
}

impl Mailbox {
    /// Create a mailbox and its paired receiver.
    pub fn channel() -> (Mailbox, Receiver<Msg>) {
        let (tx, rx) = async_channel::unbounded();
        (Mailbox { tx }, rx)
    }

    /// Enqueue a message. Never blocks; the channel is unbounded.
    pub fn send(&self, msg: Msg) {
        let _ = self.tx.try_send(msg);
    }
}

/// Executes effects produced by [`update`].
///
/// Implementors carry out side effects (engine calls, UI rendering, clipboard)
/// and may enqueue follow-up messages through `mailbox`, for example delivering
/// an async JS result back as [`Msg::JsResult`]. `state` is the post-update state
/// so render effects read current values.
pub trait EffectRunner {
    /// Perform a single effect.
    fn run(&mut self, effect: Effect, state: &State, mailbox: &Mailbox);
}

/// Apply one message and run its effects.
pub fn dispatch<R: EffectRunner>(state: &mut State, runner: &mut R, mailbox: &Mailbox, msg: Msg) {
    let effects = update(state, msg);
    for effect in effects {
        runner.run(effect, state, mailbox);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::command::{Command, ScrollDir};
    use crate::core::effect::Effect;
    use crate::core::msg::{JsPurpose, Msg};
    use crate::core::state::{Config, State};

    /// Records effects and auto-answers `ReadScrollPercent` evaluations, standing
    /// in for the engine so the full round-trip can be exercised without GTK.
    #[derive(Default)]
    struct TestRunner {
        seen: Vec<Effect>,
        scroll_answer: Option<String>,
    }

    impl EffectRunner for TestRunner {
        fn run(&mut self, effect: Effect, _state: &State, mailbox: &Mailbox) {
            if let Effect::EvalJs {
                id,
                tab,
                purpose: JsPurpose::ReadScrollPercent,
                ..
            } = &effect
                && let Some(answer) = self.scroll_answer.clone()
            {
                mailbox.send(Msg::JsResult {
                    id: *id,
                    tab: *tab,
                    result: Ok(answer),
                });
            }
            self.seen.push(effect);
        }
    }

    fn fixture() -> (State, TestRunner) {
        let mut state = State::new(Config::default());
        let id = state.tabs.open("https://example.com");
        state.tabs.focus_id(id);
        (
            state,
            TestRunner {
                scroll_answer: Some("55".to_string()),
                ..Default::default()
            },
        )
    }

    /// Drain all currently-available messages. This is the loop the GTK app runs.
    fn drain(state: &mut State, runner: &mut TestRunner, mailbox: &Mailbox, rx: &Receiver<Msg>) {
        while let Ok(msg) = rx.try_recv() {
            dispatch(state, runner, mailbox, msg);
        }
    }

    #[test]
    fn dispatch_processes_enqueued_command() {
        let (mut state, mut runner) = fixture();
        let (mailbox, rx) = Mailbox::channel();
        mailbox.send(Msg::Command(Command::Quit));
        drain(&mut state, &mut runner, &mailbox, &rx);
        assert!(!state.running);
        assert!(runner.seen.contains(&Effect::Quit));
    }

    #[test]
    fn effect_enqueued_result_is_processed_in_same_drain() {
        let (mut state, mut runner) = fixture();
        let (mailbox, rx) = Mailbox::channel();
        mailbox.send(Msg::Command(Command::Scroll(ScrollDir::Down, 1)));
        drain(&mut state, &mut runner, &mailbox, &rx);
        // The runner answered the percent read; update consumed it and set status.
        assert_eq!(state.status.scroll_percent, Some(55));
    }
}
