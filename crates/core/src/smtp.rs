//! Sending. A message goes out once or not at all, so a failure has to say which of those it
//! might have been: mail must not go twice because a retry could not tell.

use std::time::Duration;

use lettre::address::Envelope;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::imap::Credential;
use crate::store::{Account, Security};

const TIMEOUT: Duration = Duration::from_secs(60);

/// Why a message did not go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// It never left: no connection, or a temporary refusal. Safe to try again.
    NotSent(String),
    /// The server refused it for good, which needs the user to change something.
    Refused(String),
    /// The connection died while it was being handed over, so it may have gone.
    Unknown(String),
}

fn transport(account: &Account, credential: &Credential) -> Result<AsyncSmtpTransport<Tokio1Executor>, SendError> {
    let not_sent = |e: lettre::transport::smtp::Error| SendError::NotSent(e.to_string());
    let tls = match account.security {
        Security::Tls => Tls::Wrapper(TlsParameters::new_rustls(account.smtp_host.clone()).map_err(not_sent)?),
        Security::Plain if is_loopback(&account.smtp_host) => Tls::None,
        Security::Plain => return Err(SendError::Refused("refusing an unencrypted connection to a remote host".into())),
    };
    let builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&account.smtp_host)
        .port(account.smtp_port)
        .timeout(Some(TIMEOUT))
        .tls(tls);
    Ok(match credential {
        Credential::Password(password) => builder.credentials(Credentials::new(account.email.clone(), password.clone())),
        Credential::AccessToken(token) => builder
            .credentials(Credentials::new(account.email.clone(), token.clone()))
            .authentication(vec![Mechanism::Xoauth2]),
    }
    .build())
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Hands a finished message to the server. The connection is proved first, because that is the
/// failure that is common and harmless; from then on an error may mean the mail went.
pub async fn send(account: &Account, credential: &Credential, envelope: &Envelope, raw: &[u8]) -> Result<(), SendError> {
    let transport = transport(account, credential)?;
    if let Err(e) = transport.test_connection().await {
        return Err(SendError::NotSent(e.to_string()));
    }
    transport.send_raw(envelope, raw).await.map(drop).map_err(|e| {
        if e.is_transient() {
            SendError::NotSent(e.to_string())
        } else if e.is_permanent() {
            SendError::Refused(e.to_string())
        } else {
            SendError::Unknown(e.to_string())
        }
    })
}
