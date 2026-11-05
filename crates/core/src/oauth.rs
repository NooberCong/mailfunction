//! Google sign-in for a desktop app: system browser, loopback redirect, PKCE (RFC 8252).
//! Blocking by design; it runs once on its own thread, and token refreshes are rare.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

const SCOPE: &str = "https://mail.google.com/ openid email profile";
const USER_INFO: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct Client {
    pub client_id: String,
    /// Desktop secrets are not secret (RFC 8252 §8.5), but Google's token endpoint wants one.
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
}

impl Client {
    /// Reads the JSON that Google Cloud Console gives you for a Desktop client.
    pub fn from_google_json(path: &Path) -> Result<Self> {
        #[derive(Deserialize)]
        struct File {
            installed: Installed,
        }
        #[derive(Deserialize)]
        struct Installed {
            client_id: String,
            client_secret: String,
            auth_uri: String,
            token_uri: String,
        }
        let text = std::fs::read_to_string(path)?;
        let file: File = serde_json::from_str(&text)
            .map_err(|e| Error::Auth(format!("{} is not a Desktop client file: {e}", path.display())))?;
        if ![&file.installed.auth_uri, &file.installed.token_uri].iter().all(|u| u.starts_with("https://")) {
            return Err(Error::Auth("the client file's endpoints must use https".into()));
        }
        Ok(Self {
            client_id: file.installed.client_id,
            client_secret: file.installed.client_secret,
            auth_url: file.installed.auth_uri,
            token_url: file.installed.token_uri,
        })
    }
}

#[derive(Debug)]
pub struct SignedIn {
    pub email: String,
    pub refresh_token: String,
}

#[derive(Debug)]
pub struct AccessToken {
    pub token: String,
    pub expires_at: Instant,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

fn random_url_safe() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Error::Auth(format!("no system randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| s.get(i + 1..i + 3)).flatten();
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| decode(v))
    })
}

/// Answers the browser and returns the query string of the redirect. The port is reachable by
/// anything on this machine and browsers open idle sockets to it, so whatever is not the real
/// redirect is turned away without ending the wait.
fn wait_for_redirect(listener: &TcpListener, state: &str) -> Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(query) = read_redirect(stream, state) {
                    return Ok(query);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(Error::Auth("timed out waiting for the browser".into()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn read_redirect(mut stream: TcpStream, state: &str) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut line = String::new();
    BufReader::new((&stream).take(8192)).read_line(&mut line).ok()?;
    let target = line.split(' ').nth(1).unwrap_or_default();
    let query = target
        .strip_prefix("/callback?")
        .filter(|q| query_value(q, "state").as_deref() == Some(state))
        .map(str::to_string);
    let (status, body) = match query {
        Some(_) => ("200 OK", "You can close this tab and return to mailfunction."),
        None => ("404 Not Found", ""),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    query
}

fn email_from_id_token(id_token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Claims {
        email: String,
    }
    // Read, not verified: it came straight from the token endpoint over TLS (OIDC §3.1.3.7).
    let bad = |_| Error::Auth("unreadable id_token".into());
    let payload = id_token.split('.').nth(1).ok_or_else(|| Error::Auth("malformed id_token".into()))?;
    let json = URL_SAFE_NO_PAD.decode(payload).map_err(|_| Error::Auth("malformed id_token".into()))?;
    let claims: Claims = serde_json::from_slice(&json).map_err(bad)?;
    Ok(claims.email)
}

fn post_token(client: &Client, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent.post(&client.token_url).send_form(form.iter().copied())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.body_mut().read_to_string().unwrap_or_default();
        let message = format!("token endpoint said {status}: {body}");
        // 400 and 401 mean the grant is dead (invalid_grant). Anything else is the server's
        // problem, so it must be retried rather than sending the user back to sign in.
        return Err(if matches!(status.as_u16(), 400 | 401) { Error::Auth(message) } else { Error::Protocol(message) });
    }
    Ok(response.body_mut().read_json()?)
}

/// Runs the whole browser flow. `open_browser` receives the URL to show the user.
pub fn sign_in(client: &Client, open_browser: impl FnOnce(&str)) -> Result<SignedIn> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let redirect = format!("http://127.0.0.1:{}/callback", listener.local_addr()?.port());
    let verifier = random_url_safe()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_url_safe()?;
    let url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={state}\
         &code_challenge={challenge}&code_challenge_method=S256&access_type=offline&prompt=consent",
        client.auth_url,
        encode(&client.client_id),
        encode(&redirect),
        encode(SCOPE),
    );
    open_browser(&url);

    let query = wait_for_redirect(&listener, &state)?;
    let Some(code) = query_value(&query, "code") else {
        let reason = query_value(&query, "error").unwrap_or_else(|| "no code".into());
        return Err(Error::Auth(reason));
    };
    let tokens = post_token(
        client,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
            ("code_verifier", &verifier),
        ],
    )?;
    let refresh_token = tokens.refresh_token.ok_or_else(|| Error::Auth("no refresh token returned".into()))?;
    let id_token = tokens.id_token.ok_or_else(|| Error::Auth("no id_token returned".into()))?;
    Ok(SignedIn { email: email_from_id_token(&id_token)?, refresh_token })
}

/// The address of the account's profile picture, if Google gives one. A grant made before the
/// app asked for `profile` may not include it.
pub fn picture(access_token: &str) -> Result<Option<String>> {
    picture_from(USER_INFO, access_token)
}

fn picture_from(url: &str, access_token: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct UserInfo {
        picture: Option<String>,
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let mut response = agent.get(url).header("Authorization", format!("Bearer {access_token}")).call()?;
    let info: UserInfo = response.body_mut().read_json()?;
    Ok(info.picture.filter(|p| p.starts_with("https://")))
}

pub fn refresh(client: &Client, refresh_token: &str) -> Result<AccessToken> {
    let tokens = post_token(
        client,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &client.client_id),
            ("client_secret", &client.client_secret),
        ],
    )?;
    Ok(AccessToken {
        token: tokens.access_token,
        expires_at: Instant::now() + Duration::from_secs(tokens.expires_in),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// A local token endpoint: answers one POST with `status` and `body`, and hands back the
    /// form it received.
    fn token_server(status: &'static str, body: String) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut form = vec![0; length];
            reader.read_exact(&mut form).unwrap();
            tx.send(String::from_utf8(form).unwrap()).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (url, rx)
    }

    fn client(token_url: String) -> Client {
        Client {
            client_id: "id.apps".into(),
            client_secret: "shh".into(),
            auth_url: "http://auth.invalid/authorize".into(),
            token_url,
        }
    }

    fn user_info_server(body: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/userinfo", listener.local_addr().unwrap());
        let (sent, received) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 2048];
            let read = stream.read(&mut request).unwrap();
            let _ = sent.send(String::from_utf8_lossy(&request[..read]).into_owned());
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
        });
        (url, received)
    }

    #[test]
    fn a_picture_needs_the_token_and_only_https_counts() {
        let (url, request) = user_info_server(r#"{"sub":"1","picture":"https://lh3.example/a=s96-c"}"#);
        assert_eq!(picture_from(&url, "tok").unwrap().as_deref(), Some("https://lh3.example/a=s96-c"));
        assert!(request.recv().unwrap().to_lowercase().contains("authorization: bearer tok"));

        let (url, _) = user_info_server(r#"{"sub":"1"}"#);
        assert_eq!(picture_from(&url, "tok").unwrap(), None);
        let (url, _) = user_info_server(r#"{"picture":"http://lh3.example/a"}"#);
        assert_eq!(picture_from(&url, "tok").unwrap(), None);
    }

    fn id_token(email: &str) -> String {
        format!("x.{}.y", URL_SAFE_NO_PAD.encode(format!(r#"{{"email":"{email}"}}"#)))
    }

    /// Plays the browser: follows the redirect that Google would send.
    fn redirect_to(url: &str, query: impl FnOnce(&str) -> String) {
        let redirect = decode(&query_value(url.split_once('?').unwrap().1, "redirect_uri").unwrap());
        let state = query_value(url.split_once('?').unwrap().1, "state").unwrap();
        let target = format!("{redirect}?{}", query(&state));
        std::thread::spawn(move || {
            // Noise first: a stray request, a socket that closes at once, and a forged redirect.
            let host = target.split('/').nth(2).unwrap().to_string();
            for request in ["GET /favicon.ico HTTP/1.1\r\n\r\n", "", "GET /callback?state=forged&code=evil HTTP/1.1\r\n\r\n"] {
                let mut noise = TcpStream::connect(&host).unwrap();
                write!(noise, "{request}").unwrap();
                noise.shutdown(std::net::Shutdown::Write).unwrap();
                let _ = noise.read_to_end(&mut Vec::new());
            }
            let path = target.splitn(4, '/').nth(3).unwrap();
            let mut browser = TcpStream::connect(&host).unwrap();
            write!(browser, "GET /{path} HTTP/1.1\r\n\r\n").unwrap();
            let _ = browser.read_to_end(&mut Vec::new());
        });
    }

    #[test]
    fn sign_in_exchanges_the_code_with_pkce() {
        let body = format!(
            r#"{{"access_token":"a","expires_in":3600,"refresh_token":"r","id_token":"{}"}}"#,
            id_token("me@gmail.com")
        );
        let (token_url, form) = token_server("200 OK", body);
        let mut challenge = String::new();
        let signed_in = sign_in(&client(token_url), |url| {
            let query = url.split_once('?').unwrap().1;
            assert_eq!(query_value(query, "code_challenge_method").unwrap(), "S256");
            assert_eq!(query_value(query, "scope").unwrap(), SCOPE);
            assert_eq!(query_value(query, "access_type").unwrap(), "offline");
            challenge = query_value(query, "code_challenge").unwrap();
            redirect_to(url, |state| format!("state={state}&code=4%2Fabc"));
        })
        .unwrap();
        assert_eq!(signed_in.email, "me@gmail.com");
        assert_eq!(signed_in.refresh_token, "r");

        let form = form.recv().unwrap();
        assert_eq!(query_value(&form, "code").unwrap(), "4/abc", "the forged redirect's code was not used");
        assert_eq!(query_value(&form, "client_secret").unwrap(), "shh");
        let verifier = query_value(&form, "code_verifier").unwrap();
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())), challenge);
    }

    #[test]
    fn a_denied_consent_reports_why() {
        let error = sign_in(&client("http://token.invalid".into()), |url| {
            redirect_to(url, |state| format!("state={state}&error=access_denied"));
        })
        .unwrap_err();
        assert!(matches!(error, Error::Auth(m) if m == "access_denied"));
    }

    #[test]
    fn refresh_returns_a_token_and_a_revoked_grant_is_an_auth_error() {
        let (url, form) = token_server("200 OK", r#"{"access_token":"new","expires_in":3600}"#.into());
        let token = refresh(&client(url), "r").unwrap();
        assert_eq!(token.token, "new");
        assert!(form.recv().unwrap().contains("grant_type=refresh_token"));

        let (url, _form) = token_server("400 Bad Request", r#"{"error":"invalid_grant"}"#.into());
        let error = refresh(&client(url), "r").unwrap_err();
        assert!(matches!(error, Error::Auth(m) if m.contains("invalid_grant")));

        let (url, _form) = token_server("503 Service Unavailable", "busy".into());
        let error = refresh(&client(url), "r").unwrap_err();
        assert!(matches!(error, Error::Protocol(_)), "an outage must not ask the user to sign in");
    }

    #[test]
    fn reads_a_google_client_file() {
        let path = std::env::temp_dir().join(format!("mf-client-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"installed":{"client_id":"c","client_secret":"s","auth_uri":"https://a","token_uri":"https://t","redirect_uris":["http://localhost"]}}"#,
        )
        .unwrap();
        let client = Client::from_google_json(&path).unwrap();
        assert_eq!((client.client_id.as_str(), client.token_url.as_str()), ("c", "https://t"));

        std::fs::write(
            &path,
            r#"{"installed":{"client_id":"c","client_secret":"s","auth_uri":"https://a","token_uri":"http://t"}}"#,
        )
        .unwrap();
        assert!(Client::from_google_json(&path).is_err(), "tokens must not travel in the clear");
        std::fs::remove_file(&path).unwrap();
    }
}
