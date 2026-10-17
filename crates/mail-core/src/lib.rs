//! Arcalo Mail core: everything that is not window, OS or platform glue.
//!
//! The shell (`src-tauri`) only wires state, events, the credential store and the platform; the
//! logic lives here and is tested here. Nothing in this crate opens a connection except
//! [`network`], and nothing stores a password or token: secrets go to the OS credential store
//! under the names [`account::secret_key`] gives.

pub mod account;
pub mod actions;
pub mod ai;
pub mod arcalo;
pub mod applock;
pub mod autoconfig;
pub mod avatars;
pub mod calendar;
pub mod carddav;
pub mod cipher;
pub mod compose;
pub mod contacts;
pub mod crypto;
pub mod diag;
pub mod engine;
pub mod error;
pub mod files;
pub mod graph;
pub mod hub;
pub mod i18n;
pub mod imap;
pub mod itip;
pub mod jmap;
pub mod later;
pub mod mailto;
pub mod netwatch;
pub mod network;
pub mod oauth;
pub mod outbox;
pub mod parse;
pub mod phishing;
pub mod pop3;
pub mod rules;
pub mod sanitize;
pub mod search;
pub mod settings;
pub mod settings_migrate;
pub mod sevenbit;
pub mod sieve;
pub mod smtp;
pub mod store;
pub mod sync;
pub mod templates;
pub mod thread;
pub mod tracking;
pub mod triage;
pub mod unsubscribe;
pub mod update;
pub mod update_android;
pub mod update_feed;
pub mod widget;

pub use error::{Error, Result};
