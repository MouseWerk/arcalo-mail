//! The one error type of the core. Its text is shown to the user, so messages built here go
//! through `tr!`/`trf!`; errors of libraries keep their own text.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// The server answered with an error or something the client did not expect.
    #[error("{0}")]
    Imap(String),
    /// A connection was refused before it was opened (`network`), or it failed.
    #[error("{0}")]
    Network(String),
    /// Connecting or signing in failed in a way the user can act on.
    #[error("{0}")]
    Connection(crate::network::Problem),
    /// Input the user can fix (an address, a server, a port).
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    /// The OS credential store refused or is missing.
    #[error("{0}")]
    Secrets(String),
    /// Data written by a newer version, which this one does not change.
    #[error("{0}")]
    Newer(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Commands hand errors to the UI as their text.
impl Serialize for Error {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}
