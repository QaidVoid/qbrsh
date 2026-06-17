//! Browser engine abstraction.
//!
//! Browser logic depends on the [`traits::EngineView`] trait, never on WebKitGTK
//! types directly. The WebKitGTK 6 backend lives in [`webkit`] behind that trait,
//! keeping a future engine swap possible.

pub mod traits;
pub mod webkit;
