//! The window. [`Controller`] connects the Slint UI to the store and the engine. HTML mail goes
//! through [`HtmlPane`], so tests can drive all of this without a desktop.

mod actions;
pub mod badge;
pub mod chrome;
mod disc;
pub mod editor;
pub mod instance;
pub mod list;
pub mod logos;
pub mod notify;
mod settings;
pub mod startup;
pub mod tray;
pub mod webview;
mod writing;

pub use writing::save_dialog;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use list::{MessageList, display_name, full_date, initials, tone};
use logos::Logos;
use mailfunction_core::engine::{Engine, Event};
use mailfunction_core::images::{Images, Page};
use mailfunction_core::remote::Network;
use mailfunction_core::{logo, oauth, secrets};
use mailfunction_core::compose;
use mailfunction_core::render::{self, Body, Rendered};
use mailfunction_core::settings::{Identity, MarkRead, Placement, Settings};
use mailfunction_core::store::{Account, AuthKind, MessageRow, Reader, Role, Security, SidebarLabel, Store, ThreadKey};
use notify::{Notice, Notifier, Opens};
use slint::{CloseRequestResponse, Color, ComponentHandle, Image, Model, ModelRc, Timer, TimerMode, VecModel};

slint::include_modules!();

/// Bursts of sync events become at most one list refresh per interval.
const REFRESH_INTERVAL: Duration = Duration::from_millis(400);

/// Messages opened closer together than this are being flicked through, so they appear at once
/// rather than fading, which would leave the pane half-transparent while a key is held.
const FLICKING: Duration = Duration::from_millis(180);

const GMAIL_HOST: &str = "imap.gmail.com";
const IMAPS_PORT: u16 = 993;
const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";
const SMTPS_PORT: u16 = 465;

/// Mail up to this size is rendered where it is asked for. However it is built, that takes under
/// a fifth of a second, and it means ordinary mail goes up with no loading state in between.
const RENDER_IN_PLACE: usize = 64 * 1024;

/// Avatar sizes, matching app.slint, so that a logo is cut to exactly the pixels it fills.
const HEADER_AVATAR: f32 = 40.0;
const ACCOUNT_AVATAR: f32 = 28.0;
const ACCOUNT_MARK: f32 = 16.0;
const THREAD_AVATAR: f32 = 22.0;

/// A sender's picture on a notification, in pixels. Windows scales it down to fit the display.
const NOTICE_PICTURE: u32 = 128;
/// How large a sender's initials are on that picture, as a share of it.
const NOTICE_INITIALS_SCALE: f32 = 0.4;
/// More new mail than this at once becomes one notice, as after a night with the app closed.
const MOST_NOTICES: usize = 3;
/// How many senders a summary names.
const SENDERS_NAMED: usize = 3;

thread_local! {
    /// The controller, for work that finishes on another thread. `Rc` cannot cross threads,
    /// so those threads post plain data to the event loop and it is looked up here.
    static CONTROLLER: RefCell<Weak<Controller>> = const { RefCell::new(Weak::new()) };
}

pub fn on_ui_thread(work: impl FnOnce(&Rc<Controller>) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(controller) = CONTROLLER.with(|c| c.borrow().upgrade()) {
            work(&controller);
        }
    });
}

/// Replaces a model's rows, touching only those that changed. Swapping the whole model would
/// rebuild every row on each sync, dropping the hover under the pointer.
fn update_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() != rows.len() {
        model.set_vec(rows);
        return;
    }
    for (index, row) in rows.into_iter().enumerate() {
        if model.row_data(index).as_ref() != Some(&row) {
            model.set_row_data(index, row);
        }
    }
}

/// Where HTML mail goes. The real one is a webview; tests record what they are given.
pub trait HtmlPane {
    /// Starts loading a document, and sets [`Pane::Html`] once it is on screen. Until then the
    /// app shows that the message is loading. Returns false if the document cannot be shown.
    fn show(&self, app: &App, html: String) -> bool;
    /// Answers a request the page made for an image, through [`Controller::image_wanted`].
    fn image_ready(&self, ticket: u64, image: Option<Vec<u8>>);
    fn hide(&self);
    fn bounds_changed(&self, app: &App);
}

/// The remote images of the page on screen, which is what its requests are answered from. A page
/// that has none of these has nothing fetched for it, whatever it asks for.
struct PageImages {
    page: Page,
    /// Tells this page's answers from those of one the user has left.
    key: u64,
    urls: Vec<String>,
    failed: usize,
    arrived: bool,
}

/// Where the body of a message being written goes. The real one is a webview of its own, which
/// comes into view once the message is in it. Tests record what they are given.
pub trait ComposePane {
    /// Starts editing this HTML. Returns false if that cannot happen.
    fn open(&self, app: &App, html: &str, take_focus: bool) -> bool;
    fn focus(&self);
    fn release_focus(&self);
    /// Replaces the signature with this HTML, or takes it away.
    fn sign(&self, html: &str);
    fn hide(&self);
    fn bounds_changed(&self, app: &App);
}

/// How an account's connection is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Link {
    Quiet,
    Syncing,
    Offline,
    /// Sync has stopped until the user signs in again, for this reason.
    SignedOut(String),
}

struct Mailbox {
    account: Account,
    link: Link,
    identity: Identity,
    /// The identity has changes the store has not seen yet.
    unsaved: bool,
}

impl Mailbox {
    fn new(account: Account, identity: Identity) -> Self {
        Self { account, link: Link::Quiet, identity, unsaved: false }
    }
}

pub struct Controller {
    app: App,
    dir: PathBuf,
    store: Arc<Store>,
    reader: Rc<Reader>,
    list: Rc<MessageList>,
    logos: Rc<Logos>,
    badge: badge::Badge,
    tray: tray::Tray,
    /// Unread Inbox mail across every account, as the taskbar button shows it.
    unread: Cell<i64>,
    labels: RefCell<Vec<SidebarLabel>>,
    label_items: Rc<VecModel<LabelItem>>,
    /// Every signed-in account.
    accounts: RefCell<Vec<Mailbox>>,
    account_items: Rc<VecModel<AccountItem>>,
    /// The account on screen, or `None` for all of them in one list. What is done to mail goes
    /// by the account the mail is in, which only here is the same thing.
    shown: Cell<Option<i64>>,
    /// Mail on screen has changed since its list was last read.
    list_stale: Cell<bool>,
    /// The message showing in the reader.
    selected: Cell<Option<i64>>,
    /// The address the selected message is from.
    last_open: Cell<Option<Instant>>,
    /// The thread in the reader, oldest first.
    open_thread: Cell<Option<ThreadKey>>,
    thread: RefCell<Vec<MessageRow>>,
    thread_items: Rc<VecModel<ThreadItem>>,
    read_timer: Timer,
    reversible: RefCell<Option<actions::Reversible>>,
    notice_timer: Timer,
    pick_items: Rc<VecModel<PickItem>>,
    pick_ids: RefCell<Vec<i64>>,
    search_timer: Timer,
    /// Identifies the latest search, so that late answers to older ones can be told apart.
    search_key: Cell<u64>,
    /// Accounts yet to answer the latest search.
    searches_out: Cell<usize>,
    writing: RefCell<Option<writing::Writing>>,
    autosave: Timer,
    suggest_timer: Timer,
    /// A draft from elsewhere that opens in the composer once its body is here.
    opening_draft: Cell<Option<i64>>,
    pane: Box<dyn HtmlPane>,
    images: Arc<Images>,
    page_images: RefCell<Option<PageImages>>,
    page_key: Cell<u64>,
    editor: Box<dyn ComposePane>,
    notifier: Box<dyn Notifier>,
    engine: RefCell<Option<Engine>>,
    client: Option<oauth::Client>,
    refresh: Timer,
    settings: Cell<Settings>,
    /// The thread to go back to when the settings close.
    returning_to: Cell<Option<ThreadKey>>,
    identity_save: Timer,
    /// Where the window was left, once it has been put anywhere.
    placement: Cell<Option<Placement>>,
    placement_save: Timer,
}

impl Controller {
    pub fn new(
        app: App,
        dir: &Path,
        client: Option<oauth::Client>,
        pane: Box<dyn HtmlPane>,
        editor: Box<dyn ComposePane>,
        notifier: Box<dyn Notifier>,
        network: Arc<dyn Network>,
    ) -> mailfunction_core::Result<Rc<Self>> {
        let store = Arc::new(Store::open(dir)?);
        let reader = Rc::new(Reader::open(dir)?);
        let settings = reader.settings();
        app.global::<Theme>().set_preferences(settings.into());
        let images = Images::new(dir.join("images"), network.clone());
        let logos = Rc::new(Logos::new(reader.clone(), &store, network, app.as_weak()));
        logos.set_enabled(settings.sender_logos);
        let list = Rc::new(MessageList::new(reader.clone(), logos.clone()));
        app.set_rows(ModelRc::from(list.clone()));
        let label_items = Rc::new(VecModel::default());
        app.set_labels(ModelRc::from(label_items.clone()));
        let account_items = Rc::new(VecModel::default());
        app.set_accounts(ModelRc::from(account_items.clone()));
        let thread_items = Rc::new(VecModel::default());
        app.set_thread(ModelRc::from(thread_items.clone()));
        let pick_items = Rc::new(VecModel::default());
        app.set_picks(ModelRc::from(pick_items.clone()));
        let this = Rc::new(Self {
            app,
            dir: dir.to_path_buf(),
            store,
            reader,
            list,
            logos,
            badge: badge::Badge::default(),
            tray: tray::Tray::default(),
            unread: Cell::new(0),
            labels: RefCell::default(),
            label_items,
            accounts: RefCell::default(),
            account_items,
            shown: Cell::new(None),
            list_stale: Cell::new(false),
            selected: Cell::new(None),
            last_open: Cell::new(None),
            open_thread: Cell::new(None),
            thread: RefCell::default(),
            thread_items,
            read_timer: Timer::default(),
            reversible: RefCell::default(),
            notice_timer: Timer::default(),
            pick_items,
            pick_ids: RefCell::default(),
            search_timer: Timer::default(),
            search_key: Cell::new(0),
            searches_out: Cell::new(0),
            writing: RefCell::default(),
            autosave: Timer::default(),
            suggest_timer: Timer::default(),
            opening_draft: Cell::new(None),
            pane,
            images,
            page_images: RefCell::default(),
            page_key: Cell::default(),
            editor,
            notifier,
            engine: RefCell::default(),
            client,
            refresh: Timer::default(),
            settings: Cell::new(settings),
            returning_to: Cell::new(None),
            identity_save: Timer::default(),
            placement: Cell::default(),
            placement_save: Timer::default(),
        });
        CONTROLLER.with(|c| *c.borrow_mut() = Rc::downgrade(&this));
        this.connect_callbacks();
        Ok(this)
    }

    /// A callback with no arguments, which ends quietly once the controller has gone.
    fn bound(self: &Rc<Self>, f: impl Fn(&Rc<Self>) + 'static) -> impl Fn() + 'static {
        let weak = Rc::downgrade(self);
        move || {
            if let Some(this) = weak.upgrade() {
                f(&this);
            }
        }
    }

    /// As [`Controller::bound`], for a callback with one argument.
    fn bound1<A>(self: &Rc<Self>, f: impl Fn(&Rc<Self>, A) + 'static) -> impl Fn(A) + 'static {
        let weak = Rc::downgrade(self);
        move |argument| {
            if let Some(this) = weak.upgrade() {
                f(&this, argument);
            }
        }
    }

    fn connect_mail_callbacks(self: &Rc<Self>) {
        self.app.on_verb(self.bound1(|this, verb| this.verb(verb)));
        self.app.on_open_thread_message(self.bound1(|this, index| this.open_at(index as usize)));
        self.app.on_save_attachment(self.bound1(|this, index| this.save_attachment(index as usize)));
        self.app.on_pick_labels(self.bound(|this| this.pick_labels()));
        self.app.on_toggle_label(self.bound1(|this, index| this.toggle_label(index as usize)));
        self.app.on_go_to(self.bound1(|this, kind| this.go_to(kind)));
        self.app.on_leave_message(self.bound(|this| {
            this.clear_message();
        }));
        self.app.on_search_edited(self.bound(|this| this.search_edited()));
        self.app.on_search_closed(self.bound(|this| this.close_search()));
        self.app.on_draft_edited(self.bound(|this| this.draft_edited()));
        let weak = Rc::downgrade(self);
        self.app.on_address_edited(move |field, text| {
            if let Some(this) = weak.upgrade() {
                this.address_edited(field, &text);
            }
        });
        self.app.on_accept_suggestion(self.bound1(|this, index| this.accept_suggestion(index as usize)));
        self.app.on_choose_sender(self.bound1(|this, index| this.choose_sender(index as usize)));
        self.app.on_show_copies(self.bound(|this| this.app.set_draft_copies(true)));
        self.app.on_send_draft(self.bound(|this| this.send_draft()));
        self.app.on_remove_draft_file(self.bound1(|this, index| this.remove_draft_file(index as usize)));
        self.app.on_focus_editor(self.bound(|this| this.editor.focus()));
        self.app.on_editor_moved(self.bound(|this| {
            this.editor.bounds_changed(&this.app);
            this.app.invoke_repaint();
        }));
        self.app.on_close_draft(self.bound(|this| this.finish_writing()));
        self.app.on_notice_clicked(self.bound(|this| this.notice_clicked()));
    }

    fn connect_callbacks(self: &Rc<Self>) {
        self.connect_mail_callbacks();
        self.connect_settings_callbacks();
        let weak = Rc::downgrade(self);
        self.app.window().on_close_requested(move || {
            if let Some(this) = weak.upgrade() {
                this.close_window();
            }
            CloseRequestResponse::KeepWindowShown
        });
        let weak = Rc::downgrade(self);
        self.app.on_select_label(move |index| {
            if let Some(this) = weak.upgrade() {
                this.close_search();
                this.select_label(index as usize);
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_select_row(move |row| {
            if let Some(this) = weak.upgrade() {
                this.select_row(row as usize);
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_sign_in(move || {
            if let Some(this) = weak.upgrade() {
                this.sign_in();
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_add_account(move || {
            if let Some(this) = weak.upgrade() {
                // Sign-in covers the window, and nothing covers a webview but moving it away.
                this.finish_writing();
                this.open_sign_in("");
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_sign_in_again(move || {
            if let Some(this) = weak.upgrade() {
                this.finish_writing();
                this.sign_in_again();
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_sync_now(move || {
            if let Some(this) = weak.upgrade() {
                this.sync_now();
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_cancel_add_account(move || {
            if let Some(this) = weak.upgrade() {
                this.close_sign_in();
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_select_account(move |index| {
            if let Some(this) = weak.upgrade() {
                this.select_account(index as usize);
            }
        });
        self.app.on_select_all_accounts(self.bound(|this| this.select_all_accounts()));
        let weak = Rc::downgrade(self);
        self.app.on_remove_account(move |index| {
            if let Some(this) = weak.upgrade() {
                this.remove_account(index as usize);
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_load_images(move || {
            if let Some(this) = weak.upgrade() {
                this.load_images();
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_always_load_images(move || {
            if let Some(this) = weak.upgrade() {
                this.set_images_allowed(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_stop_loading_images(move || {
            if let Some(this) = weak.upgrade() {
                this.set_images_allowed(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_slot_changed(move || {
            if let Some(this) = weak.upgrade() {
                this.pane.bounds_changed(&this.app);
                this.app.invoke_repaint();
            }
        });
        let weak = Rc::downgrade(self);
        self.app.on_theme_changed(move || {
            if let Some(this) = weak.upgrade() {
                chrome::paint_title_bar(&this.app);
                this.reload_editor();
            }
        });
    }

    /// Reads the accounts from the store and shows their mail.
    pub fn load_accounts(&self) -> mailfunction_core::Result<()> {
        let mut mailboxes = Vec::new();
        for account in self.reader.accounts()? {
            let identity = self.reader.identity(account.id)?;
            mailboxes.push(Mailbox::new(account, identity));
        }
        *self.accounts.borrow_mut() = mailboxes;
        self.show_everything();
        Ok(())
    }

    /// Shows all the mail there is, or the sign-in screen if there are no accounts.
    fn show_everything(&self) {
        let accounts: Vec<i64> = self.accounts.borrow().iter().map(|m| m.account.id).collect();
        self.app.set_signed_in(!accounts.is_empty());
        match accounts[..] {
            [] => {
                self.shown.set(None);
                self.clear_message();
                self.publish_accounts();
                self.app.set_entrance_key(self.app.get_entrance_key() + 1);
            }
            [only] => self.show(Some(only)),
            _ => self.show(None),
        }
    }

    fn show(&self, account: Option<i64>) {
        self.shown.set(account);
        self.list.mark_accounts(if account.is_some() { Vec::new() } else { self.accounts_on_screen() });
        self.labels.borrow_mut().clear();
        self.reload_labels();
        self.select_label(0);
        self.publish_accounts();
    }

    fn on_screen(&self, account: i64) -> bool {
        self.shown.get().is_none_or(|shown| shown == account)
    }

    fn accounts_on_screen(&self) -> Vec<i64> {
        self.accounts.borrow().iter().map(|m| m.account.id).filter(|id| self.on_screen(*id)).collect()
    }

    /// Shows another account. What was in progress belongs to the one being left, so it ends
    /// there first.
    pub fn select_account(self: &Rc<Self>, index: usize) {
        let id = self.accounts.borrow().get(index).map(|m| m.account.id);
        if id.is_some() {
            self.switch_to(id);
        }
    }

    /// Shows every account's mail together, which takes more than one to mean anything.
    pub fn select_all_accounts(self: &Rc<Self>) {
        if self.accounts.borrow().len() > 1 {
            self.switch_to(None);
        }
    }

    fn switch_to(self: &Rc<Self>, account: Option<i64>) {
        if account != self.shown.get() {
            self.finish_writing();
            self.close_search();
            self.show(account);
        }
    }

    /// Starts background sync for every account, and for any added later.
    pub fn start_engine(self: &Rc<Self>) {
        let listener = |account, event| on_ui_thread(move |this| this.on_event(account, event));
        let mut engine = Engine::start(self.dir.clone(), self.store.clone(), self.client.clone(), listener);
        for mailbox in self.accounts.borrow().iter() {
            engine.add(mailbox.account.clone());
        }
        *self.engine.borrow_mut() = Some(engine);
    }

    /// Adds an account, or refreshes one signing in again, and shows it.
    pub fn add_account(&self, account: Account) -> mailfunction_core::Result<()> {
        let id = self.store.add_account(&account)?;
        let account = Account { id, ..account };
        if let Some(engine) = self.engine.borrow_mut().as_mut() {
            engine.add(account.clone());
        }
        {
            let mut accounts = self.accounts.borrow_mut();
            let mailbox = Mailbox::new(account, self.reader.identity(id)?);
            match accounts.iter_mut().find(|m| m.account.id == id) {
                Some(existing) => *existing = mailbox,
                None => accounts.push(mailbox),
            }
        }
        self.app.set_signed_in(true);
        self.show(Some(id));
        Ok(())
    }

    /// Removes an account: stops its sync and deletes its mail and credentials. The delete
    /// runs on its own thread, as a large mailbox takes a while to go.
    pub fn remove_account(self: &Rc<Self>, index: usize) {
        self.finish_writing();
        self.close_search();
        let removed = {
            let mut accounts = self.accounts.borrow_mut();
            if index >= accounts.len() {
                return;
            }
            accounts.remove(index).account
        };
        if let Some(engine) = self.engine.borrow_mut().as_mut() {
            engine.remove(removed.id);
        }
        self.notifier.forget(removed.id);
        let (store, images) = (self.store.clone(), self.images.clone());
        std::thread::spawn(move || {
            // If either fails the account returns at the next launch, asking to sign in again.
            // Best effort, as being offline must not stop an account going. Gone from here but
            // alive at Google, a copy of the token would still open the whole mailbox.
            if removed.auth == AuthKind::Google
                && let Ok(token) = secrets::load(&removed.email)
            {
                let _ = oauth::revoke(&token);
            }
            let _ = secrets::delete(&removed.email);
            let _ = store.remove_account(removed.id);
            images.clear();
        });
        if self.accounts.borrow().is_empty() {
            self.leave_settings();
        }
        if self.on_screen(removed.id) {
            self.show_everything();
        } else {
            self.publish_accounts();
        }
    }

    /// Covers the mail with the sign-in screen. The webview sits above everything else in the
    /// window, so it has to go first.
    fn open_sign_in(&self, message: &str) {
        self.hide_page();
        if self.client.is_some() {
            self.app.set_sign_in_message(message.into());
        }
        self.app.set_adding(true);
        self.app.set_entrance_key(self.app.get_entrance_key() + 1);
    }

    fn sign_in_again(&self) {
        let reason = self.accounts.borrow().iter().find_map(|m| match &m.link {
            Link::SignedOut(reason) if self.on_screen(m.account.id) => {
                Some(format!("Sign in to {} again.\n{reason}", m.account.email))
            }
            _ => None,
        });
        self.open_sign_in(&reason.unwrap_or_default());
    }

    fn close_sign_in(self: &Rc<Self>) {
        self.app.set_adding(false);
        if let Ok(row) = usize::try_from(self.app.get_selected_row()) {
            self.select_row(row);
        }
    }

    /// Runs the browser sign-in on its own thread, since it waits on the user.
    fn sign_in(&self) {
        let Some(client) = self.client.clone() else { return };
        self.app.set_can_sign_in(false);
        self.app.set_sign_in_message("Continue in your browser.".into());
        std::thread::spawn(move || {
            let result = oauth::sign_in(&client, webview::open_externally).and_then(|signed_in| {
                secrets::save(&signed_in.email, &signed_in.refresh_token)?;
                Ok(signed_in.email)
            });
            on_ui_thread(move |this| this.finish_sign_in(result));
        });
    }

    fn finish_sign_in(self: &Rc<Self>, result: mailfunction_core::Result<String>) {
        self.app.set_can_sign_in(true);
        let added = result.and_then(|email| {
            self.add_account(Account {
                id: 0,
                email,
                host: GMAIL_HOST.into(),
                port: IMAPS_PORT,
                security: Security::Tls,
                auth: AuthKind::Google,
                smtp_host: GMAIL_SMTP_HOST.into(),
                smtp_port: SMTPS_PORT,
            })
        });
        match added {
            Ok(()) => {
                self.app.set_sign_in_message("".into());
                self.app.set_adding(false);
            }
            Err(e) => self.app.set_sign_in_message(e.to_string().into()),
        }
    }

    pub fn on_event(self: &Rc<Self>, account: i64, event: Event) {
        match event {
            Event::Changed => self.schedule_refresh(account),
            Event::Arrived(messages) => self.announce(account, &messages),
            Event::Syncing => self.set_link(account, Link::Syncing),
            Event::UpToDate => {
                self.set_link(account, Link::Quiet);
                self.schedule_refresh(account);
            }
            Event::Offline => self.set_link(account, Link::Offline),
            Event::SignInRequired(reason) => self.set_link(account, Link::SignedOut(reason)),
            // Message ids are unique across accounts, so these need no account check.
            Event::BodyReady(id) if self.selected.get() == Some(id) => self.body_ready(id),
            Event::BodyFailed(id) if self.selected.get() == Some(id) => self.app.set_pane(Pane::Failed),
            Event::BodyReady(_) | Event::BodyFailed(_) => {}
            Event::ActionRefused => {
                self.show_notice("The server said no, so that was put back", "", Duration::from_secs(8));
                self.schedule_refresh(account);
            }
            Event::Sent(id) => {
                self.sent(id);
                self.schedule_refresh(account);
            }
            Event::SendFailed { maybe, reason, .. } => {
                self.send_failed(maybe, &reason);
                self.schedule_refresh(account);
            }
            Event::SearchDone { key, ids } => self.search_done(key, &ids),
            Event::AccountPicture => {
                let email = self.accounts.borrow().iter().find(|m| m.account.id == account).map(|m| m.account.email.clone());
                if let Some(email) = email {
                    self.logo_ready(&logo::picture_key(&email));
                }
            }
        }
    }

    /// Tells the user about new mail. Only unread Inbox mail is news: the rest is their own sent
    /// mail, spam, or mail they have already read somewhere else.
    fn announce(&self, account: i64, arrived: &[i64]) {
        let settings = self.settings.get();
        let messages = self.reader.unread_in_inbox(arrived).unwrap_or_default();
        let accounts = self.accounts.borrow();
        let Some(mailbox) = accounts.iter().find(|m| m.account.id == account) else { return };
        if !settings.notifications || !mailbox.identity.notify || messages.is_empty() {
            return;
        }
        let address = mailbox.account.email.clone();
        let silent = !settings.notification_sound;
        if messages.len() > MOST_NOTICES || !settings.notification_preview {
            let mut senders: Vec<&str> = Vec::new();
            for name in messages.iter().map(display_name).filter(|_| settings.notification_preview) {
                if senders.len() < SENDERS_NAMED && !senders.contains(&name) {
                    senders.push(name);
                }
            }
            let one = (messages.len() == 1).then(|| messages[0].id);
            self.notifier.show(&Notice {
                title: if one.is_some() { "New email".into() } else { format!("{} new emails", messages.len()) },
                text: senders.join(", "),
                account: address,
                picture: None,
                silent,
                opens: Opens { account, message: one },
            });
            return;
        }
        // Oldest first, so that the newest ends up on top of the stack.
        for message in messages.iter().rev() {
            let view = header(message);
            self.notifier.show(&Notice {
                title: view.sender.to_string(),
                text: view.subject.to_string(),
                account: address.clone(),
                picture: self.sender_picture(message, &view),
                silent,
                opens: Opens { account, message: Some(message.id) },
            });
        }
    }

    /// The sender's logo, or else their initials in the colours the list gives them.
    fn sender_picture(&self, message: &MessageRow, view: &MessageView) -> Option<Vec<u8>> {
        let domain = logo::sender_domain(&message.from_addr).filter(|_| message.verified);
        let logo = domain.and_then(|domain| self.logos.png(&domain, NOTICE_PICTURE));
        logo.or_else(|| {
            let tones = self.app.global::<Theme>().get_tones();
            let tone = tones.row_data(view.tone as usize % tones.row_count())?;
            let rgb = |colour: Color| [colour.red(), colour.green(), colour.blue()];
            let (back, ink) = (rgb(tone.back), rgb(tone.ink));
            let pixels = disc::draw(&view.initials, NOTICE_PICTURE, back, ink, NOTICE_INITIALS_SCALE);
            logo::png(pixels.into_flattened(), NOTICE_PICTURE)
        })
    }

    /// Closing the window leaves the app running, so that mail keeps arriving, unless the user
    /// would rather it did not. The browser goes either way, as it is most of the app's memory
    /// and nobody is looking at it.
    fn close_window(&self) {
        self.save_identities();
        self.save_placement();
        chrome::hide(&self.app);
        self.hide_page();
        self.editor.hide();
        if !self.settings.get().keep_running {
            let _ = slint::quit_event_loop();
        }
    }

    /// Brings the window up, opening again the page that closing it let go of.
    pub fn reveal(self: &Rc<Self>) {
        let returning = chrome::raise(&self.app);
        if returning {
            self.reload_editor();
        }
        if returning
            && !self.app.get_composing()
            && matches!(self.app.get_pane(), Pane::Html | Pane::Loading)
            && let Ok(row) = usize::try_from(self.app.get_selected_row())
        {
            self.select_row(row);
        }
    }

    /// Brings up what a clicked notification is about.
    pub fn open(self: &Rc<Self>, opens: Opens) {
        self.reveal();
        if !self.accounts.borrow().iter().any(|m| m.account.id == opens.account) {
            return;
        }
        self.app.set_adding(false);
        self.leave_settings();
        self.finish_writing();
        self.close_search();
        if !self.on_screen(opens.account) {
            self.show(Some(opens.account));
        }
        let inbox = self.labels.borrow().iter().position(|l| l.role == Role::Inbox).unwrap_or_default();
        self.select_label(inbox);
        // The list is of threads, so the message is found by the thread it is in.
        let thread = opens.message.and_then(|id| self.reader.rows(&[id]).ok()?.first().map(MessageRow::thread_key));
        if let Some(row) = thread.and_then(|thread| self.list_position(thread)) {
            self.select_row(row);
            self.app.invoke_reveal(row as i32);
        }
    }

    fn set_link(&self, account: i64, link: Link) {
        if let Some(mailbox) = self.accounts.borrow_mut().iter_mut().find(|m| m.account.id == account) {
            mailbox.link = link;
        }
        self.publish_accounts();
    }

    /// Syncs what is on screen now. The engine reports back with [`Event::Syncing`].
    fn sync_now(&self) {
        if let Some(engine) = self.engine.borrow().as_ref() {
            for account in self.accounts_on_screen() {
                engine.sync_now(account);
            }
        }
    }

    /// Brings the sidebar's accounts and the state of what is on screen up to date.
    fn publish_accounts(&self) {
        let unread = self.reader.inbox_unread().unwrap_or_default();
        let accounts = self.accounts.borrow();
        self.unread.set(accounts.iter().filter_map(|m| unread.get(&m.account.id)).sum());
        self.app.set_all_unread(if self.unread.get() > 0 { self.unread.get().to_string().into() } else { "".into() });
        self.show_badge();
        let items = accounts
            .iter()
            .enumerate()
            .map(|(index, m)| AccountItem {
                email: m.account.email.as_str().into(),
                initials: initials(&m.account.email).into(),
                // By place, as a colour is all that tells two accounts apart on a row of the list.
                tone: index as i32,
                logo: self.account_logo(&m.account.email),
                picture: self.logos.stored(&logo::picture_key(&m.account.email), ACCOUNT_MARK),
                unread: unread.get(&m.account.id).map_or_else(Default::default, |n| n.to_string().into()),
                status: match m.link {
                    Link::Offline => "Offline".into(),
                    Link::SignedOut(_) => "Needs sign-in".into(),
                    Link::Quiet | Link::Syncing => "".into(),
                },
                name: m.identity.name.as_str().into(),
                signature: m.identity.signature.as_str().into(),
                notify: m.identity.notify,
            })
            .collect();
        update_rows(&self.account_items, items);
        let active = accounts.iter().position(|m| Some(m.account.id) == self.shown.get());
        self.app.set_active_account(active.map_or(-1, |i| i as i32));
        let links = || accounts.iter().filter(|m| self.on_screen(m.account.id)).map(|m| &m.link);
        self.app.set_syncing(links().any(|link| *link == Link::Syncing));
        self.app.set_needs_sign_in(links().any(|link| matches!(link, Link::SignedOut(_))));
    }

    fn show_badge(&self) {
        if let Some(hwnd) = chrome::hwnd(&self.app) {
            self.badge.show(hwnd, self.taskbar_count());
            self.tray.show(hwnd, self.taskbar_count());
        }
    }

    /// The number on the taskbar and in the notification area.
    pub fn taskbar_count(&self) -> i64 {
        if self.settings.get().taskbar_count { self.unread.get() } else { 0 }
    }

    /// Puts the count back on a taskbar button that has just appeared.
    pub fn refresh_badge(&self) {
        self.badge.forget();
        self.show_badge();
    }

    /// Puts the icon back in a notification area that has just restarted.
    pub fn restore_tray(&self) {
        self.tray.forget();
        self.show_badge();
    }

    /// Unread Inbox mail across every account.
    pub fn unread(&self) -> i64 {
        self.unread.get()
    }

    /// The account's own picture if it has one, or else its provider's logo.
    fn account_logo(&self, email: &str) -> Image {
        let picture = self.logos.stored(&logo::picture_key(email), ACCOUNT_AVATAR);
        if picture.size().width > 0 {
            return picture;
        }
        logo::account_domain(email).map_or_else(Image::default, |domain| self.logos.image(&domain, ACCOUNT_AVATAR))
    }

    fn schedule_refresh(self: &Rc<Self>, account: i64) {
        if self.on_screen(account) {
            self.list_stale.set(true);
        }
        if self.refresh.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.refresh.start(TimerMode::SingleShot, REFRESH_INTERVAL, move || {
            let Some(this) = weak.upgrade() else { return };
            // An account in the background moves only its unread count.
            if this.list_stale.take() {
                this.refresh_now();
            } else {
                this.publish_accounts();
            }
        });
    }

    /// Reloads what is on screen, keeping the selection on the same message.
    pub fn refresh_now(&self) {
        self.publish_accounts();
        let first_labels = self.labels.borrow().is_empty();
        self.reload_labels();
        if first_labels {
            // A new account has no labels until its first sync reports some.
            self.select_label(0);
            return;
        }
        self.list.refresh();
        let row = self.open_thread.get().and_then(|thread| self.list_position(thread));
        self.app.set_selected_row(row.map_or(-1, |r| r as i32));
        if self.writing.borrow().is_none() {
            self.reload_thread();
        }
    }

    /// Where a thread is in the list on screen.
    fn list_position(&self, thread: ThreadKey) -> Option<usize> {
        if self.app.get_searching() {
            return self.list.result_position(thread);
        }
        self.reader.position(&self.current_labels(), thread).ok().flatten()
    }

    /// The labels behind the sidebar line on screen, as (account id, label id).
    pub(crate) fn current_held(&self) -> Vec<(i64, i64)> {
        self.labels.borrow().get(self.app.get_active_label() as usize).map_or_else(Vec::new, |l| l.held.clone())
    }

    fn current_labels(&self) -> Vec<i64> {
        self.current_held().into_iter().map(|(_, label)| label).collect()
    }

    fn reload_labels(&self) {
        let accounts = self.accounts_on_screen();
        if accounts.is_empty() {
            return;
        }
        let current = self.current_labels();
        let labels = self.reader.sidebar(&accounts).unwrap_or_default();
        let items: Vec<_> = labels
            .iter()
            .map(|l| LabelItem {
                name: l.name.as_str().into(),
                // Unread counts only mean something where mail waits to be read.
                unread: if l.unread > 0 && matches!(l.role, Role::Inbox | Role::None) {
                    l.unread.to_string().into()
                } else {
                    "".into()
                },
                kind: label_kind(l.role),
            })
            .collect();
        let first_custom = labels.iter().position(|l| l.role == Role::None).unwrap_or(labels.len());
        self.app.set_first_custom_label(first_custom as i32);
        if let Some(index) = labels.iter().position(|l| l.ids().iter().any(|id| current.contains(id))) {
            self.app.set_active_label(index as i32);
            self.list.set_labels(labels[index].ids());
        }
        *self.labels.borrow_mut() = labels;
        update_rows(&self.label_items, items);
    }

    pub fn select_label(&self, index: usize) {
        // A new account has no labels until it syncs, and no labels is no mail.
        let (ids, role) = self.labels.borrow().get(index).map_or((Vec::new(), Role::None), |l| (l.ids(), l.role));
        self.app.set_active_label(index as i32);
        self.list.show_labels(ids, role != Role::Spam);
        self.app.invoke_scroll_to_top();
        self.app.set_list_key(self.app.get_list_key() + 1);
        self.clear_message();
    }

    fn clear_message(&self) {
        self.read_timer.stop();
        self.selected.set(None);
        self.opening_draft.set(None);
        self.open_thread.set(None);
        self.thread.borrow_mut().clear();
        self.thread_items.set_vec(Vec::new());
        self.app.set_selected_row(-1);
        self.app.set_image_bar(ImageBar::Hidden);
        self.app.set_pane(Pane::Empty);
        self.app.set_picking(false);
        self.hide_page();
    }

    fn current_role(&self) -> Role {
        if self.app.get_searching() {
            return Role::None;
        }
        self.labels.borrow().get(self.app.get_active_label() as usize).map_or(Role::None, |l| l.role)
    }

    /// Opens what a row of the list stands for: a thread, or in Drafts and the Outbox, the
    /// composer. Mail in the Outbox stops waiting and can be changed again.
    pub fn select_row(self: &Rc<Self>, row: usize) {
        let Some(head) = self.list.message(row) else { return };
        self.finish_writing();
        self.app.set_selected_row(row as i32);
        self.opening_draft.set(None);
        match self.current_role() {
            Role::Drafts | Role::Outbox => self.open_draft(head.id),
            _ if self.reader.draft(head.id).ok().flatten().is_some() => self.open_draft(head.id),
            _ => self.open_thread(&head),
        }
    }

    fn thread_rows(&self, head: &MessageRow) -> Vec<MessageRow> {
        let binned = matches!(self.current_role(), Role::Trash | Role::Spam);
        let rows = self.reader.thread(head.account_id, head.thread_id, binned).ok();
        rows.filter(|rows| !rows.is_empty()).unwrap_or_else(|| vec![head.clone()])
    }

    /// Opens a thread at the first message still to read, or else at its newest.
    fn open_thread(self: &Rc<Self>, head: &MessageRow) {
        let thread = self.thread_rows(head);
        let index = thread.iter().position(|m| !m.seen).unwrap_or(thread.len() - 1);
        self.open_thread.set(Some(head.thread_key()));
        *self.thread.borrow_mut() = thread;
        self.publish_thread();
        self.open_at(index);
        let delay = match self.settings.get().mark_read {
            MarkRead::OnOpening => actions::READ_DELAY,
            MarkRead::AfterAWhile => actions::READ_LATER,
            MarkRead::Never => return,
        };
        let weak = Rc::downgrade(self);
        self.read_timer.start(TimerMode::SingleShot, delay, move || {
            if let Some(this) = weak.upgrade() {
                this.mark_read();
            }
        });
    }

    /// Picks up changes to the open thread without disturbing the message on show.
    fn reload_thread(&self) {
        let Some(head) = self.thread.borrow().first().cloned() else { return };
        let thread = self.thread_rows(&head);
        let open = self.selected.get().and_then(|id| thread.iter().position(|m| m.id == id));
        *self.thread.borrow_mut() = thread;
        self.publish_thread();
        self.app.set_thread_open(open.map_or(-1, |i| i as i32));
    }

    /// The account the open thread is in.
    pub(crate) fn open_account(&self) -> Option<i64> {
        self.thread.borrow().first().map(|m| m.account_id)
    }

    fn publish_thread(&self) {
        let thread = self.thread.borrow();
        let items: Vec<ThreadItem> = thread
            .iter()
            .map(|m| ThreadItem {
                sender: display_name(m).into(),
                date: list::short_date(m.date, chrono::Local::now()).into(),
                initials: initials(display_name(m)).into(),
                tone: tone(&m.from_addr),
                logo: self.list.logo(m, THREAD_AVATAR),
                unread: !m.seen,
            })
            .collect();
        update_rows(&self.thread_items, items);
        self.app.set_starred(thread.iter().any(|m| m.flagged));
        self.app.set_can_label(thread.first().is_some_and(|m| self.reader.has_role(m.account_id, Role::All).unwrap_or(false)));
        let role = self.current_role();
        self.app.set_in_trash(role == Role::Trash);
        self.app.set_in_spam(role == Role::Spam);
        drop(thread);
        self.app.set_in_inbox(self.thread_in_inbox());
    }

    /// Shows one message of the open thread.
    fn open_at(&self, index: usize) {
        let Some(message) = self.thread.borrow().get(index).cloned() else { return };
        self.app.set_thread_open(index as i32);
        self.selected.set(Some(message.id));
        self.app.set_image_bar(ImageBar::Hidden);
        // The old message goes in the same frame as the click, whatever the new one takes.
        self.hide_page();
        let mut view = header(&message);
        view.logo = self.list.logo(&message, HEADER_AVATAR);
        self.app.set_message(view);
        let now = Instant::now();
        if self.last_open.replace(Some(now)).is_none_or(|last| now - last > FLICKING) {
            self.app.set_header_key(self.app.get_header_key() + 1);
            self.app.set_body_key(self.app.get_body_key() + 1);
        }
        match self.reader.has_body(message.id) {
            Ok(true) => self.show_body(message.id),
            _ => self.fetch_selected_body(message.id),
        }
    }

    /// Puts a message back in the reader after something covered it.
    fn reopen(&self, id: i64) {
        let index = self.thread.borrow().iter().position(|m| m.id == id);
        if let Some(index) = index {
            self.open_at(index);
        }
    }

    fn fetch_selected_body(&self, id: i64) {
        self.app.set_pane(Pane::Loading);
        let account = self.reader.account_of(id).ok().flatten();
        let asked = account.zip(self.engine.borrow().as_ref()).map(|(account, engine)| engine.fetch_body(account, id));
        if asked == Some(false) {
            self.app.set_pane(Pane::Failed);
        }
    }

    /// A body that was asked for has arrived.
    fn body_ready(self: &Rc<Self>, id: i64) {
        if self.opening_draft.take() == Some(id) {
            self.open_draft(id);
        } else {
            self.show_body(id);
        }
    }

    fn show_body(&self, id: i64) {
        self.show_page(id, self.all_images_load(id) || self.sender_allowed(id));
    }

    /// Puts a message up. Most go up in the frame they were asked for, and any remote images come
    /// in behind them. Cleaning hostile HTML can take a long time, so past a size where that could
    /// be noticed it happens on a thread of its own, and the window carries on.
    fn show_page(&self, id: i64, load_images: bool) {
        let render = if load_images { render::message_with_images } else { render::message };
        let Some(raw) = self.reader.body(id).ok().flatten() else {
            return self.page_rendered(self.next_page(), id, load_images, Vec::new(), None);
        };
        let key = self.next_page();
        if raw.len() <= RENDER_IN_PLACE {
            let rendered = render(&raw);
            return self.page_rendered(key, id, load_images, raw, rendered);
        }
        self.app.set_pane(Pane::Loading);
        std::thread::spawn(move || {
            let rendered = render(&raw);
            on_ui_thread(move |this| this.page_rendered(key, id, load_images, raw, rendered));
        });
    }

    /// A key for the page about to go up. Any page still being rendered is no longer wanted.
    fn next_page(&self) -> u64 {
        self.page_key.set(self.page_key.get() + 1);
        self.page_key.get()
    }

    fn page_rendered(&self, key: u64, id: i64, load_images: bool, raw: Vec<u8>, rendered: Option<Rendered>) {
        if key != self.page_key.get() {
            return;
        }
        let Some(rendered) = rendered else {
            self.hide_page();
            self.app.set_pane(Pane::Failed);
            return;
        };
        let bar = match (rendered.remote_images.is_empty(), load_images) {
            (true, _) => ImageBar::Hidden,
            (false, false) => ImageBar::Blocked,
            (false, true) if !self.all_images_load(id) && self.sender_allowed(id) => ImageBar::Allowed,
            (false, true) => ImageBar::Hidden,
        };
        self.app.set_can_always_load(self.trusted_row(id).is_some());
        self.page_images.replace(load_images.then(|| PageImages {
            page: self.images.page(),
            key,
            urls: rendered.remote_images.clone(),
            failed: 0,
            arrived: false,
        }));
        self.display(&raw, rendered, bar);
    }

    /// Takes the page away, along with whatever it was still waiting for.
    fn hide_page(&self) {
        self.next_page();
        self.page_images.take();
        self.pane.hide();
    }

    /// The message of the open thread that is on show.
    fn selected_row(&self) -> Option<MessageRow> {
        let id = self.selected.get()?;
        self.thread.borrow().iter().find(|m| m.id == id).cloned()
    }

    fn in_spam(&self, id: i64) -> bool {
        let held = self.reader.message_labels(id).unwrap_or_default();
        self.labels.borrow().iter().any(|l| l.role == Role::Spam && l.ids().iter().any(|id| held.contains(id)))
    }

    /// The message on show, if it can speak for its sender, which a forgery of them must not.
    /// Spam is where a failed forgery ends up, so nothing there can.
    fn trusted_row(&self, id: i64) -> Option<MessageRow> {
        self.selected_row().filter(|m| m.id == id && m.verified && !self.in_spam(id))
    }

    fn sender_allowed(&self, id: i64) -> bool {
        self.trusted_row(id).is_some_and(|m| self.reader.images_allowed(m.account_id, &m.from_addr).unwrap_or(false))
    }

    /// Images load without asking, except in Spam, where loading one tells a spammer the address
    /// is read. Searches reach Spam too, so this goes by the message and not by the list on show.
    fn all_images_load(&self, id: i64) -> bool {
        self.settings.get().show_all_images && !self.in_spam(id)
    }

    /// Shows the selected message with its images, because the user asked for them.
    fn load_images(&self) {
        if let Some(id) = self.selected.get() {
            self.show_page(id, true);
        }
    }

    fn set_images_allowed(&self, allowed: bool) {
        let Some(message) = self.selected_row() else { return };
        let saved = self.store.set_images_allowed(message.account_id, &message.from_addr, allowed);
        match (saved.is_ok(), allowed, self.selected.get()) {
            (true, true, _) => self.load_images(),
            (true, false, Some(id)) => self.show_body(id),
            _ => {}
        }
    }

    /// The page asks for one of its images. The webview has no network, so it is fetched here.
    pub fn image_wanted(&self, index: usize, ticket: u64) {
        let images = self.page_images.borrow();
        let Some((images, url)) = images.as_ref().and_then(|i| Some((i, i.urls.get(index)?))) else {
            return self.pane.image_ready(ticket, None);
        };
        let key = images.key;
        images.page.get(url.clone(), move |image| {
            on_ui_thread(move |this| this.image_fetched(key, ticket, image));
        });
    }

    /// Hands an image to the page. A page that gets none at all says so, for trying again.
    pub fn image_fetched(&self, key: u64, ticket: u64, image: Option<Vec<u8>>) {
        let found = image.is_some();
        self.pane.image_ready(ticket, image);
        let mut images = self.page_images.borrow_mut();
        let Some(images) = images.as_mut().filter(|i| i.key == key) else { return };
        images.arrived |= found;
        images.failed += usize::from(!found);
        if !images.arrived && images.failed == images.urls.len() {
            self.app.set_image_bar(ImageBar::Failed);
        }
    }

    /// The key that answers for the page on screen, if it loads images.
    pub fn page_key(&self) -> Option<u64> {
        self.page_images.borrow().as_ref().map(|i| i.key)
    }

    pub fn selected_message(&self) -> Option<i64> {
        self.selected.get()
    }

    /// Fits a logo that has just arrived wherever its domain is on screen.
    pub fn logo_ready(&self, domain: &str) {
        self.logos.forget(domain);
        self.list.logo_changed(domain);
        self.publish_accounts();
        if let Some(message) = self.selected_row().filter(|m| logo::sender_domain(&m.from_addr).as_deref() == Some(domain)) {
            let mut view = self.app.get_message();
            view.logo = self.list.logo(&message, HEADER_AVATAR);
            self.app.set_message(view);
        }
    }

    fn display(&self, raw: &[u8], rendered: Rendered, bar: ImageBar) {
        self.app.set_image_bar(bar);
        let mut view = self.app.get_message();
        view.recipients = rendered.recipients.into();
        let files: Vec<FileItem> = compose::attachments(raw)
            .into_iter()
            .map(|file| writing::file_item(&file.name, file.data.len()))
            .collect();
        view.attachments = ModelRc::new(VecModel::from(files));
        match rendered.body {
            Body::Text(text) => {
                self.hide_page();
                view.text = text.into();
                self.app.set_message(view);
                self.app.set_pane(Pane::Text);
            }
            Body::Html(html) => {
                self.app.set_message(view);
                self.app.set_pane(Pane::Loading);
                if !self.pane.show(&self.app, html) {
                    self.app.set_pane(Pane::Failed);
                }
            }
        }
    }
}

fn label_kind(role: Role) -> LabelKind {
    match role {
        Role::Inbox => LabelKind::Inbox,
        Role::Starred => LabelKind::Starred,
        Role::Sent => LabelKind::Sent,
        Role::Drafts => LabelKind::Drafts,
        Role::Outbox => LabelKind::Outbox,
        Role::All | Role::Archive => LabelKind::All,
        Role::Spam => LabelKind::Spam,
        Role::Trash => LabelKind::Trash,
        Role::None => LabelKind::Custom,
    }
}

fn header(m: &MessageRow) -> MessageView {
    let name = display_name(m);
    MessageView {
        subject: if m.subject.is_empty() { "(no subject)".into() } else { m.subject.as_str().into() },
        sender: name.into(),
        address: if name == m.from_addr { "".into() } else { m.from_addr.as_str().into() },
        date: full_date(m.date).into(),
        initials: initials(name).into(),
        tone: tone(&m.from_addr),
        logo: Image::default(),
        recipients: "".into(),
        attachments: ModelRc::default(),
        text: "".into(),
    }
}
