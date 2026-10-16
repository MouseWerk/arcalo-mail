//! Arcalo Mail core: everything that is not window, OS or platform glue.
//!
//! The shell (`src-tauri`) only wires state, events, the credential store and the platform; the
//! logic lives here and is tested here. Nothing in this crate opens a connection except
//! [`network`], and nothing stores a password or token: secrets go to the OS credential store
//! under the names [`account::secret_key`] gives.

pub mod account;
pub mod actions;
pub mod autoconfig;
pub mod compose;
pub mod diag;
pub mod engine;
pub mod error;
pub mod i18n;
pub mod imap;
pub mod jmap;
pub mod netwatch;
pub mod network;
pub mod oauth;
pub mod outbox;
pub mod parse;
pub mod sanitize;
pub mod search;
pub mod settings;
pub mod settings_migrate;
pub mod sevenbit;
pub mod smtp;
pub mod store;
pub mod sync;
pub mod thread;

pub use error::{Error, Result};
