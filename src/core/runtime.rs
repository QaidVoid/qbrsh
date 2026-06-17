//! The message queue and single-consumer dispatch loop.
//!
//! A [`Runtime`] owns the sole [`State`] and a [`Mailbox`]. Every source of
//! change enqueues a [`Msg`] on the mailbox; [`Runtime::pump`] drains them one at
//! a time, applying each through [`update`] and handing the resulting effects to
//! an [`EffectRunner`]. Because mutation happens only inside the drain, no two
//! mutations overlap and there is no re-entrancy: an effect that would trigger
//! more work enqueues new messages rather than re-entering `update`.
//!
//! The mailbox uses interior mutability for the *queue*, not for application
//! state. The queue is single-threaded here; the worker-thread bridge that feeds
//! it from other threads is added with the async/worker subsystem.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::core::effect::Effect;
use crate::core::msg::Msg;
use crate::core::state::State;
use crate::core::update::update;

/// A cloneable handle for enqueuing messages onto the runtime's queue.
#[derive(Clone, Default)]
pub struct Mailbox {
    queue: Rc<RefCell<VecDeque<Msg>>>,
}

impl Mailbox {
    /// Create an empty mailbox.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a message for the dispatch loop.
    pub fn send(&self, msg: Msg) {
        self.queue.borrow_mut().push_back(msg);
    }

    /// Remove the next message, if any.
    fn pop(&self) -> Option<Msg> {
        self.queue.borrow_mut().pop_front()
    }

    /// Whether the queue currently holds messages.
    pub fn has_pending(&self) -> bool {
        !self.queue.borrow().is_empty()
    }
}

/// Executes effects produced by [`update`].
///
/// Implementors carry out side effects (engine calls, UI rendering, clipboard,
/// etc.) and may enqueue follow-up messages through the provided [`Mailbox`] —
/// for example, delivering an async JS result back as [`Msg::JsResult`].
pub trait EffectRunner {
    /// Perform a single effect, using `mailbox` to report any asynchronous result.
    fn run(&mut self, effect: Effect, mailbox: &Mailbox);
}

/// Owns the application state and drives the message loop.
pub struct Runtime<R: EffectRunner> {
    state: State,
    mailbox: Mailbox,
    runner: R,
}

impl<R: EffectRunner> Runtime<R> {
    /// Create a runtime over the given initial state and effect runner.
    pub fn new(state: State, runner: R) -> Self {
        Self {
            state,
            mailbox: Mailbox::new(),
            runner,
        }
    }

    /// A handle for enqueuing messages from event sources.
    pub fn mailbox(&self) -> Mailbox {
        self.mailbox.clone()
    }

    /// Read-only access to the current state.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Whether the application is still running.
    pub fn running(&self) -> bool {
        self.state.running
    }

    /// Drain all currently-queued messages, including any enqueued by effects
    /// during this drain. Each message is applied with exclusive `&mut State`,
    /// one at a time.
    pub fn pump(&mut self) {
        while let Some(msg) = self.mailbox.pop() {
            let effects = update(&mut self.state, msg);
            for effect in effects {
                self.runner.run(effect, &self.mailbox);
            }
        }
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
        fn run(&mut self, effect: Effect, mailbox: &Mailbox) {
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

    fn runtime() -> Runtime<TestRunner> {
        let mut state = State::new(Config::default());
        state.tabs.open("https://example.com");
        state.tabs.focus_last();
        Runtime::new(
            state,
            TestRunner {
                scroll_answer: Some("55".to_string()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn pump_processes_enqueued_command() {
        let mut rt = runtime();
        rt.mailbox().send(Msg::Command(Command::Quit));
        rt.pump();
        assert!(!rt.running());
        assert!(rt.runner.seen.contains(&Effect::Quit));
    }

    #[test]
    fn effect_enqueued_result_is_processed_in_same_drain() {
        let mut rt = runtime();
        rt.mailbox()
            .send(Msg::Command(Command::Scroll(ScrollDir::Down, 1)));
        rt.pump();
        // The runner answered the percent read; update consumed it and set status.
        assert_eq!(rt.state().status.scroll_percent, Some(55));
        assert!(!rt.mailbox().has_pending());
    }

    #[test]
    fn messages_drain_to_empty() {
        let mut rt = runtime();
        let mb = rt.mailbox();
        mb.send(Msg::Command(Command::Scroll(ScrollDir::Down, 1)));
        mb.send(Msg::Command(Command::Scroll(ScrollDir::Up, 1)));
        rt.pump();
        assert!(!mb.has_pending());
    }
}
