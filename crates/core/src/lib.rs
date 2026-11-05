//! Everything that is not UI: the local store, IMAP sync, the operation queue, sending, sign-in
//! and message rendering.
//!
//! The UI reads only from the store through a [`store::Reader`] and hears about changes from
//! the [`engine::Engine`], so it never waits on the network.

pub mod compose;
pub mod draft;
pub mod engine;
pub mod imap;
pub mod logo;
pub mod oauth;
pub mod ops;
pub mod remote;
pub mod render;
pub mod secrets;
pub mod settings;
pub mod smtp;
pub mod store;
pub mod sync;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("imap: {0}")]
    Imap(#[from] async_imap::error::Error),
    #[error("network: {0}")]
    Io(#[from] std::io::Error),
    #[error("http: {0}")]
    Http(#[from] ureq::Error),
    #[error("credential store: {0}")]
    Keyring(#[from] keyring_core::Error),
    /// The server or user refused the credentials. Retrying will not help; sign in again.
    #[error("sign-in failed: {0}")]
    Auth(String),
    /// The server understood and said no. Trying the same thing again will not help.
    #[error("refused: {0}")]
    Refused(String),
    #[error("{0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;
