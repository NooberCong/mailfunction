//! The background side of the app: one thread for all accounts, running three tasks for each.
//!
//! - The sync task owns one connection: it replays the user's actions, syncs every folder, then
//!   IDLEs on the main one. Actions share this connection on purpose: nothing can move a message
//!   while a pass is deciding what has gone.
//! - The body task owns a second connection, so opening a message or searching the server never
//!   waits behind a long sync. RFC 4549 suggests this split, and Gmail allows 15 connections.
//! - The outbox task sends mail over SMTP when its time comes.
//!
//! The work is almost all waiting on sockets, so one thread is enough however many accounts
//! there are, and an account adds sockets rather than a thread.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, mpsc};
use tokio::task::LocalSet;

use crate::imap::{Credential, Imap};
use crate::logo;
use crate::oauth::{self, AccessToken};
use crate::smtp::{self, SendError};
use crate::store::{Account, AuthKind, Reader, Store};
use crate::sync::sync_account;
use crate::{Error, Result, secrets};

/// How long to IDLE before checking the folders IDLE cannot see.
const IDLE_LIMIT: Duration = Duration::from_secs(5 * 60);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
const PREFETCH_MAX_SIZE: u32 = 2 * 1024 * 1024;
const PREFETCH_RETRY: Duration = Duration::from_secs(30);
/// How long a profile picture is kept before asking whether it has changed.
const PICTURE_RECHECK_SECONDS: i64 = 7 * 24 * 3600;
/// How long to wait before trying again to send mail that could not leave.
const SEND_RETRY_SECONDS: i64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The store has new data for lists.
    Changed,
    /// These messages have just come in.
    Arrived(Vec<i64>),
    Syncing,
    UpToDate,
    Offline,
    /// The account needs signing in again, for the given reason; sync has stopped.
    SignInRequired(String),
    /// A requested body is now in the store.
    BodyReady(i64),
    /// A requested body could not be fetched.
    BodyFailed(i64),
    /// The account's profile picture is in the store.
    AccountPicture,
    /// The server refused something the user did, and the store has put it back.
    ActionRefused,
    /// A message has gone.
    Sent(i64),
    /// A message did not go and is back in Drafts. `maybe` means it may have gone all the same.
    SendFailed { message: i64, maybe: bool, reason: String },
    /// What the server found for a search, as message ids, newest first.
    SearchDone { key: u64, ids: Vec<i64> },
}

type Listener = Arc<dyn Fn(Event) + Send + Sync>;

/// Hands out credentials, refreshing the OAuth access token when it is close to expiry.
struct Credentials {
    account: Account,
    client: Option<oauth::Client>,
    token: Mutex<Option<AccessToken>>,
}

impl Credentials {
    async fn get(self: &Arc<Self>) -> Result<Credential> {
        let email = self.account.email.clone();
        if self.account.auth == AuthKind::Password {
            return Ok(Credential::Password(secrets::load(&email)?));
        }
        let this = self.clone();
        let task = tokio::task::spawn_blocking(move || {
            let mut token = this.token.lock().expect("token lock");
            let fresh = token.as_ref().is_some_and(|t| t.expires_at > Instant::now() + Duration::from_secs(60));
            if !fresh {
                let client = this.client.as_ref().ok_or_else(|| Error::Auth("no OAuth client configured".into()))?;
                *token = Some(oauth::refresh(client, &secrets::load(&email)?)?);
            }
            Ok::<_, Error>(token.as_ref().expect("token").token.clone())
        });
        Ok(Credential::AccessToken(task.await.map_err(|e| Error::Protocol(e.to_string()))??))
    }
}

/// What the body task can be asked for.
enum Request {
    Body(i64),
    /// A body a notification is waiting for. Unlike the one on screen, it never goes out of date.
    NoticeBody(i64),
    Search { key: u64, query: String },
}

/// How the UI gets an account's tasks moving.
#[derive(Default)]
struct Signals {
    /// Sync now, because the user asked.
    sync: Notify,
    /// There are actions for the server.
    queue: Notify,
    /// There is mail for the outbox.
    outbox: Notify,
}

/// One account's work, on its way to the engine thread.
struct Job {
    credentials: Arc<Credentials>,
    requests: mpsc::UnboundedReceiver<Request>,
    signals: Arc<Signals>,
}

/// The UI's side of an account's tasks. Dropping it ends them.
struct Running {
    requests: mpsc::UnboundedSender<Request>,
    signals: Arc<Signals>,
}

/// A handle to the background thread, which runs for as long as this does.
pub struct Engine {
    jobs: mpsc::UnboundedSender<Job>,
    running: HashMap<i64, Running>,
    client: Option<oauth::Client>,
}

impl Engine {
    /// Starts with no accounts. The listener hears each event with its account's id.
    pub fn start(
        dir: PathBuf,
        store: Arc<Store>,
        client: Option<oauth::Client>,
        listener: impl Fn(i64, Event) + Send + Sync + 'static,
    ) -> Self {
        let (jobs, mut waiting) = mpsc::unbounded_channel::<Job>();
        std::thread::Builder::new()
            .name("mail-engine".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                let listener = Arc::new(listener);
                LocalSet::new().block_on(&runtime, async move {
                    let Ok(reader) = Reader::open(&dir) else { return };
                    let reader = Rc::new(reader);
                    while let Some(job) = waiting.recv().await {
                        let id = job.credentials.account.id;
                        let shared = listener.clone();
                        let events: Listener = Arc::new(move |event| shared(id, event));
                        tokio::task::spawn_local(run_account(job, store.clone(), reader.clone(), events));
                    }
                });
            })
            .expect("engine thread");
        Self { jobs, running: HashMap::new(), client }
    }

    /// Starts syncing an account, restarting it if it was already running.
    pub fn add(&mut self, account: Account) {
        let (sender, requests) = mpsc::unbounded_channel();
        let signals = Arc::new(Signals::default());
        let id = account.id;
        let credentials = Arc::new(Credentials { account, client: self.client.clone(), token: Mutex::new(None) });
        let _ = self.jobs.send(Job { credentials, requests, signals: signals.clone() });
        self.running.insert(id, Running { requests: sender, signals });
    }

    /// Stops an account's tasks and closes its connections.
    pub fn remove(&mut self, account_id: i64) {
        self.running.remove(&account_id);
    }

    /// Asks for a message's body. Answered with [`Event::BodyReady`] or [`Event::BodyFailed`],
    /// unless this returns false because the account has stopped, so no answer will come.
    pub fn fetch_body(&self, account_id: i64, message_id: i64) -> bool {
        self.running.get(&account_id).is_some_and(|r| r.requests.send(Request::Body(message_id)).is_ok())
    }

    /// Asks for the body of a message whose notification is waiting for it, answered as
    /// [`Self::fetch_body`] is.
    pub fn fetch_notice_body(&self, account_id: i64, message_id: i64) -> bool {
        self.running.get(&account_id).is_some_and(|r| r.requests.send(Request::NoticeBody(message_id)).is_ok())
    }

    /// Asks the server to search, for what is not stored here. Answered with
    /// [`Event::SearchDone`], unless this returns false because the account has stopped.
    pub fn search(&self, account_id: i64, key: u64, query: String) -> bool {
        self.running.get(&account_id).is_some_and(|r| r.requests.send(Request::Search { key, query }).is_ok())
    }

    /// Ends the account's current wait and syncs now.
    pub fn sync_now(&self, account_id: i64) {
        if let Some(running) = self.running.get(&account_id) {
            running.signals.sync.notify_one();
        }
    }

    /// Tells the account's tasks that the store has new work for the server.
    pub fn flush(&self, account_id: i64) {
        if let Some(running) = self.running.get(&account_id) {
            running.signals.queue.notify_one();
            running.signals.outbox.notify_one();
        }
    }
}

/// One account's tasks. Any ending stops the rest: sync ends when sign-in fails, and the body
/// task when the UI drops the account.
async fn run_account(job: Job, store: Arc<Store>, reader: Rc<Reader>, listener: Listener) {
    let Job { credentials, requests, signals } = job;
    tokio::task::spawn_local(account_picture(store.clone(), reader.clone(), credentials.clone(), listener.clone()));
    if store.resume_drafts(credentials.account.id).is_err() {
        return;
    }
    let prefetch = Notify::new();
    tokio::select! {
        () = sync_task(&store, &credentials, &listener, &signals, &prefetch) => {}
        () = body_task(&reader, &store, &credentials, &listener, requests, &prefetch) => {}
        () = outbox_task(&store, &credentials, &listener, &signals) => {}
    }
}

/// Keeps the account's profile picture current. It only decorates the sidebar, so if anything
/// goes wrong the account keeps the logo it has, and sync is what reports a sign-in problem.
async fn account_picture(store: Arc<Store>, reader: Rc<Reader>, credentials: Arc<Credentials>, listener: Listener) {
    if credentials.account.auth != AuthKind::Google {
        return;
    }
    let key = logo::picture_key(&credentials.account.email);
    let now = chrono::Utc::now().timestamp();
    if reader.logo(&key).ok().flatten().is_some_and(|(_, checked)| now - checked < PICTURE_RECHECK_SECONDS) {
        return;
    }
    let Ok(Credential::AccessToken(token)) = credentials.get().await else { return };
    let fetched = tokio::task::spawn_blocking(move || {
        let url = oauth::picture(&token).ok()??;
        logo::picture(&url)
    });
    if let Ok(Some(png)) = fetched.await
        && store.save_logo(&key, Some(&png), now).is_ok()
    {
        listener(Event::AccountPicture);
    }
}

async fn connect(credentials: &Arc<Credentials>) -> Result<Imap> {
    let credential = credentials.get().await?;
    Imap::connect(&credentials.account, &credential).await
}

async fn sync_task(
    store: &Store,
    credentials: &Arc<Credentials>,
    listener: &Listener,
    signals: &Signals,
    prefetch: &Notify,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match sync_session(store, credentials, listener, signals, prefetch).await {
            Err(Error::Auth(reason)) => {
                listener(Event::SignInRequired(reason));
                return;
            }
            // Sessions end routinely (Gmail closes them near token expiry), so only a
            // connection that died quickly counts as being offline.
            Err(_) if started.elapsed() < Duration::from_secs(30) => {
                listener(Event::Offline);
                let _ = tokio::time::timeout(backoff, signals.sync.notified()).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
            _ => backoff = Duration::from_secs(1),
        }
    }
}

/// Runs until the connection fails. Each loop is one full pass and one wait.
async fn sync_session(
    store: &Store,
    credentials: &Arc<Credentials>,
    listener: &Listener,
    signals: &Signals,
    prefetch: &Notify,
) -> Result<()> {
    let mut imap = connect(credentials).await?;
    // A pass that only carries the user's own action is not worth a spinner.
    let mut quietly = false;
    loop {
        if !quietly {
            listener(Event::Syncing);
        }
        let folders = sync_account(&mut imap, store, credentials.account.id, |event| {
            if event == Event::Changed {
                prefetch.notify_one();
            }
            listener(event);
        })
        .await?;
        if !quietly {
            listener(Event::UpToDate);
        }

        // IDLE only reports what happens from now on, and mail may have landed in this folder
        // while the others were syncing.
        let status = imap.examine(&folders[0].path).await?;
        if status.uid_next > folders[0].next_uid || status.highest_modseq > folders[0].modseq {
            continue;
        }
        (imap, quietly) = imap.idle(IDLE_LIMIT, &signals.sync, &signals.queue).await?;
    }
}

/// Sends mail as it falls due. Whatever was going when the app last stopped comes first.
async fn outbox_task(store: &Store, credentials: &Arc<Credentials>, listener: &Listener, signals: &Signals) {
    let account = credentials.account.id;
    for message in store.interrupted_sends(account).unwrap_or_default() {
        if store.fail_send(message).is_ok() {
            let reason = "The app closed while it was sending.".to_string();
            listener(Event::SendFailed { message, maybe: true, reason });
        }
    }
    loop {
        let now = chrono::Utc::now().timestamp();
        match store.next_send(account) {
            Ok(Some((message, due))) if due <= now => send(store, credentials, listener, signals, message).await,
            Ok(Some((_, due))) => {
                let wait = Duration::from_secs((due - now) as u64);
                let _ = tokio::time::timeout(wait, signals.outbox.notified()).await;
            }
            // The store is local, so an error here is not one that passes with a retry.
            Ok(None) | Err(_) => signals.outbox.notified().await,
        }
    }
}

async fn send(store: &Store, credentials: &Arc<Credentials>, listener: &Listener, signals: &Signals, message: i64) {
    let retry = || {
        let _ = store.retry_send(message, chrono::Utc::now().timestamp() + SEND_RETRY_SECONDS);
        listener(Event::Changed);
    };
    let prepared = match store.begin_send(message) {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return,
        Err(e) => {
            let _ = store.fail_send(message);
            listener(Event::SendFailed { message, maybe: false, reason: e.to_string() });
            return;
        }
    };
    let Ok(credential) = credentials.get().await else {
        retry();
        return;
    };
    match smtp::send(&credentials.account, &credential, &prepared.envelope, &prepared.raw).await {
        Ok(()) => {
            let _ = store.finish_send(message, &prepared.raw);
            listener(Event::Sent(message));
            signals.queue.notify_one();
        }
        Err(SendError::NotSent(_)) => retry(),
        Err(SendError::Refused(reason)) => {
            let _ = store.fail_send(message);
            listener(Event::SendFailed { message, maybe: false, reason });
        }
        Err(SendError::Unknown(reason)) => {
            let _ = store.fail_send(message);
            listener(Event::SendFailed { message, maybe: true, reason });
        }
    }
}

async fn body_task(
    reader: &Reader,
    store: &Store,
    credentials: &Arc<Credentials>,
    listener: &Listener,
    mut requests: mpsc::UnboundedReceiver<Request>,
    prefetch: &Notify,
) {
    let mut fetcher = BodyFetcher { imap: None, open: None };
    let mut prefetch_paused = false;
    // Skipped for the rest of the session, so that one bad message cannot block those behind it.
    let mut prefetch_failed = HashSet::new();
    loop {
        let next_prefetch = if prefetch_paused {
            None
        } else {
            // Bodies load on demand, and only the newest Inbox messages are fetched ahead.
            let newest = reader.settings().offline_messages as usize;
            let missing = reader.missing_bodies(credentials.account.id, PREFETCH_MAX_SIZE, newest);
            missing.unwrap_or_default().into_iter().find(|id| !prefetch_failed.contains(id))
        };
        let request = match next_prefetch {
            // Requests jump the queue: prefetching only runs when nobody is waiting.
            Some(id) => match requests.try_recv() {
                Ok(request) => request,
                Err(mpsc::error::TryRecvError::Disconnected) => return,
                Err(mpsc::error::TryRecvError::Empty) => {
                    if fetcher.fetch(id, reader, store, credentials).await.is_err() {
                        prefetch_failed.insert(id);
                        prefetch_paused = true;
                    }
                    continue;
                }
            },
            None => tokio::select! {
                request = requests.recv() => match request {
                    Some(request) => request,
                    None => return,
                },
                // New mail: look again now rather than at the next retry.
                () = prefetch.notified() => continue,
                () = tokio::time::sleep(PREFETCH_RETRY) => {
                    prefetch_paused = false;
                    continue;
                }
            },
        };
        let (body, notices, search) = latest(request, &mut requests);
        for id in body.into_iter().chain(notices) {
            let ok = fetcher.fetch(id, reader, store, credentials).await.is_ok();
            listener(if ok { Event::BodyReady(id) } else { Event::BodyFailed(id) });
        }
        if let Some((key, query)) = search {
            let ids = fetcher.search(&query, reader, credentials).await.unwrap_or_default();
            listener(Event::SearchDone { key, ids });
        }
    }
}

/// Only the most recent body and search matter, as the user has moved on from the others.
fn latest(first: Request, requests: &mut mpsc::UnboundedReceiver<Request>) -> (Option<i64>, Vec<i64>, Option<(u64, String)>) {
    let (mut body, mut notices, mut search) = (None, Vec::new(), None);
    let mut next = Some(first);
    while let Some(request) = next {
        match request {
            Request::Body(id) => body = Some(id),
            Request::NoticeBody(id) => notices.push(id),
            Request::Search { key, query } => search = Some((key, query)),
        }
        next = requests.try_recv().ok();
    }
    (body, notices, search)
}

struct BodyFetcher {
    imap: Option<Imap>,
    open: Option<String>,
}

impl BodyFetcher {
    async fn fetch(&mut self, id: i64, reader: &Reader, store: &Store, credentials: &Arc<Credentials>) -> Result<()> {
        if reader.has_body(id)? {
            return Ok(());
        }
        let (path, uid) = reader.location(id)?.ok_or_else(|| Error::Protocol("message has no location".into()))?;
        // One retry, because the idle connection may have been closed since the last use, and a
        // folder opened before this message arrived does not show it until opened again.
        for attempt in 0..2 {
            match self.fetch_once(&path, uid, credentials).await {
                Ok(Some(raw)) => {
                    store.save_body(id, &raw)?;
                    return Ok(());
                }
                Ok(None) if attempt == 0 => self.open = None,
                Ok(None) => return Err(Error::Protocol(format!("UID {uid} is gone from {path}"))),
                Err(e) if attempt == 1 => return Err(e),
                Err(_) => {
                    self.imap = None;
                    self.open = None;
                }
            }
        }
        unreachable!("the second attempt returns")
    }

    async fn fetch_once(&mut self, path: &str, uid: u32, credentials: &Arc<Credentials>) -> Result<Option<Vec<u8>>> {
        self.open(path, credentials).await?.fetch_body(uid).await
    }

    /// The connection, with a folder open on it, connecting if it has to.
    async fn open(&mut self, path: &str, credentials: &Arc<Credentials>) -> Result<&mut Imap> {
        if self.imap.is_none() {
            self.imap = Some(connect(credentials).await?);
        }
        let imap = self.imap.as_mut().expect("connected");
        if self.open.as_deref() != Some(path) {
            imap.examine(path).await?;
            self.open = Some(path.to_string());
        }
        Ok(imap)
    }

    /// Searches every folder that counts, for messages we hold. Anything else is history that
    /// has not synced yet, which has no row to show.
    async fn search(&mut self, query: &str, reader: &Reader, credentials: &Arc<Credentials>) -> Result<Vec<i64>> {
        for attempt in 0..2 {
            match self.search_once(query, reader, credentials).await {
                Err(e) if attempt == 1 => return Err(e),
                Err(_) => {
                    self.imap = None;
                    self.open = None;
                }
                found => return found,
            }
        }
        unreachable!("the second attempt returns")
    }

    async fn search_once(&mut self, query: &str, reader: &Reader, credentials: &Arc<Credentials>) -> Result<Vec<i64>> {
        let account = credentials.account.id;
        let mut ids = Vec::new();
        for path in reader.searchable_folders(account)? {
            for uid in self.open(&path, credentials).await?.search_text(query).await? {
                ids.extend(reader.message_at(account, &path, uid)?);
            }
        }
        Ok(ids)
    }
}
