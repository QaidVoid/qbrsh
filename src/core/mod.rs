//! The hand-rolled Elm-style (TEA) core of qbrsh.
//!
//! All application state lives in a single owned [`state::State`]. Every source
//! of change produces a [`msg::Msg`] that is drained by one consumer through the
//! pure [`update::update`] function, which mutates state and returns a list of
//! [`effect::Effect`] values. Effects are executed by an [`runtime::EffectRunner`]
//! after `update` returns; effects that produce results report them back as new
//! messages. This keeps state mutation in one place, free of shared interior
//! mutability and re-entrancy.

// The core defines the complete Msg/Effect/Command vocabulary up front. Variants
// for engine and input events are constructed as those subsystems are wired in;
// this allowance is removed during the final cleanup once all sources exist.
#![allow(dead_code)]

pub mod bindings;
pub mod command;
pub mod completion;
pub mod effect;
pub mod key;
pub mod msg;
pub mod runtime;
pub mod state;
pub mod trie;
pub mod update;
