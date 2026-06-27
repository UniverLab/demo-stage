//! The lifecycle commands: capture, open, stop, record, export, source, scene, focus.
//!
//! `normalize` is no longer a user command — it runs automatically at the end of
//! `capture` — but its logic lives in [`crate::normalize`] and is invoked here.

pub mod capture;
pub mod control;
pub mod doctor;
pub mod edit;
pub mod export;
pub mod focus;
pub mod normalize;
pub mod open;
pub mod record;
pub mod scene;
pub mod source;
pub mod stop;
