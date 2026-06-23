//! The lifecycle commands: prepare, capture, record, check, export.
//!
//! `normalize` is no longer a user command — it runs automatically at the end of
//! `capture` — but its logic lives in [`crate::normalize`] and is invoked here.

pub mod capture;
pub mod check;
pub mod export;
pub mod normalize;
pub mod prepare;
pub mod record;
pub mod stop;
