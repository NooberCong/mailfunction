//! The message list as a Slint model. Rows come from the store in small pages as the list
//! scrolls them into view, so a mailbox of any size costs a few pages of memory.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Datelike, Local, TimeZone};
use mailfunction_core::logo;
use mailfunction_core::store::{MessageRow, Reader};
use slint::{Image, Model, ModelNotify, ModelTracker};

use crate::RowItem;
use crate::logos::Logos;

const PAGE: usize = 100;
const MAX_PAGES: usize = 8;
/// The size of a row's avatar, matching `Avatar` in app.slint.
const AVATAR: f32 = 38.0;

pub struct MessageList {
    reader: Rc<Reader>,
    label: Cell<i64>,
    count: Cell<usize>,
    pages: RefCell<HashMap<usize, Rc<Vec<MessageRow>>>>,
    logos: Rc<Logos>,
    /// False in Spam: a lookup would tell a spammer's domain that the address is alive, and a
    /// familiar logo is the last thing a forgery should get.
    show_logos: Cell<bool>,
    /// What a search found, which is shown instead of the label while there is a search.
    results: RefCell<Option<Vec<MessageRow>>>,
    notify: ModelNotify,
}

impl MessageList {
    pub fn new(reader: Rc<Reader>, logos: Rc<Logos>) -> Self {
        Self {
            reader,
            label: Cell::new(0),
            count: Cell::new(0),
            pages: RefCell::default(),
            logos,
            show_logos: Cell::new(true),
            results: RefCell::default(),
            notify: ModelNotify::default(),
        }
    }

    pub fn show_label(&self, label: i64, show_logos: bool) {
        self.label.set(label);
        self.show_logos.set(show_logos);
        self.results.borrow_mut().take();
        self.pages.borrow_mut().clear();
        self.count.set(self.reader.message_count(label).unwrap_or(0));
        self.notify.reset();
    }

    pub fn show_results(&self, rows: Vec<MessageRow>) {
        self.show_logos.set(true);
        self.count.set(rows.len());
        *self.results.borrow_mut() = Some(rows);
        self.notify.reset();
    }

    /// Adds what the server found to the results, newest first, leaving out threads already there.
    pub fn add_results(&self, rows: Vec<MessageRow>) {
        let mut results = self.results.borrow_mut();
        let Some(results) = results.as_mut() else { return };
        let before = results.len();
        for row in rows {
            if !results.iter().any(|r| r.thread_id == row.thread_id) {
                results.push(row);
            }
        }
        if results.len() != before {
            results.sort_by_key(|r| std::cmp::Reverse(r.date));
            self.count.set(results.len());
            self.notify.reset();
        }
    }

    /// Takes a thread out of the results. A list of a label sees this for itself.
    pub fn drop_thread(&self, thread: Option<i64>) {
        let mut results = self.results.borrow_mut();
        if let Some(results) = results.as_mut() {
            results.retain(|r| Some(r.thread_id) != thread);
            self.count.set(results.len());
            self.notify.reset();
        }
    }

    pub fn result_position(&self, thread: i64) -> Option<usize> {
        self.results.borrow().as_ref()?.iter().position(|r| r.thread_id == thread)
    }

    /// The logo of a message's sender, for an avatar of `size` logical pixels.
    pub fn logo(&self, address: &str, size: f32) -> Image {
        match logo::sender_domain(address) {
            Some(domain) if self.show_logos.get() => self.logos.image(&domain, size),
            _ => Image::default(),
        }
    }

    /// Redraws the rows from a domain whose logo has just arrived.
    pub fn logo_changed(&self, domain: &str) {
        for (index, page) in self.pages.borrow().iter() {
            for (offset, row) in page.iter().enumerate() {
                if logo::sender_domain(&row.from_addr).as_deref() == Some(domain) {
                    self.notify.row_changed(index * PAGE + offset);
                }
            }
        }
    }

    /// Picks up changes in the store. With the same number of rows only their contents change,
    /// so the list stays where it is, and the row under the pointer keeps its hover.
    pub fn refresh(&self) {
        let fresh = {
            let results = self.results.borrow();
            results.as_ref().map(|rows| {
                let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
                self.reader.rows(&ids).unwrap_or_default()
            })
        };
        if let Some(rows) = fresh {
            self.count.set(rows.len());
            *self.results.borrow_mut() = Some(rows);
            self.notify.reset();
            return;
        }
        let shown: Vec<usize> = self.pages.borrow().keys().copied().collect();
        self.pages.borrow_mut().clear();
        let count = self.reader.message_count(self.label.get()).unwrap_or(0);
        if self.count.replace(count) != count {
            self.notify.reset();
            return;
        }
        for row in shown.into_iter().flat_map(|page| page * PAGE..(page + 1) * PAGE).filter(|row| *row < count) {
            self.notify.row_changed(row);
        }
    }

    fn page(&self, index: usize) -> Rc<Vec<MessageRow>> {
        if let Some(page) = self.pages.borrow().get(&index) {
            return page.clone();
        }
        let rows = self.reader.messages(self.label.get(), index * PAGE, PAGE).unwrap_or_default();
        let page = Rc::new(rows);
        let mut pages = self.pages.borrow_mut();
        if pages.len() >= MAX_PAGES {
            pages.clear();
        }
        pages.insert(index, page.clone());
        page
    }

    pub fn message(&self, row: usize) -> Option<MessageRow> {
        if let Some(results) = self.results.borrow().as_ref() {
            return results.get(row).cloned();
        }
        self.page(row / PAGE).get(row % PAGE).cloned()
    }
}

impl Model for MessageList {
    type Data = RowItem;

    fn row_count(&self) -> usize {
        self.count.get()
    }

    fn row_data(&self, row: usize) -> Option<RowItem> {
        let m = self.message(row)?;
        Some(RowItem {
            sender: display_name(&m).into(),
            subject: if m.subject.is_empty() { "(no subject)".into() } else { m.subject.as_str().into() },
            date: short_date(m.date, Local::now()).into(),
            unread: !m.seen,
            starred: m.flagged,
            initials: initials(display_name(&m)).into(),
            tone: tone(&m.from_addr),
            logo: self.logo(&m.from_addr, AVATAR),
            count: if m.count > 1 { m.count.to_string().into() } else { "".into() },
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }
}

pub fn display_name(m: &MessageRow) -> &str {
    if m.from_name.is_empty() { &m.from_addr } else { &m.from_name }
}

/// The first letters of the first and last words, or one letter for a single word or an address.
pub fn initials(name: &str) -> String {
    let first_letter = |word: &str| word.chars().find(|c| c.is_alphanumeric());
    let name = name.split('@').next().unwrap_or(name);
    let mut words = name.split_whitespace().filter(|w| first_letter(w).is_some());
    let first = words.next().and_then(first_letter);
    let last = words.next_back().and_then(first_letter);
    first.into_iter().chain(last).flat_map(char::to_uppercase).collect()
}

/// A colour index that stays the same for an address, so people are recognisable in the list.
pub fn tone(address: &str) -> i32 {
    let hash = address.bytes().fold(0x811c_9dc5_u32, |hash, byte| (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193));
    (hash >> 8 & 0xffff) as i32
}

/// Time for today, day and month for this year, and the full date for anything older.
pub fn short_date<Tz: TimeZone>(timestamp: i64, now: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(date) = now.timezone().timestamp_opt(timestamp, 0).single() else {
        return String::new();
    };
    if date.date_naive() == now.date_naive() {
        date.format("%H:%M").to_string()
    } else if date.year() == now.year() {
        date.format("%-d %b").to_string()
    } else {
        date.format("%-d %b %Y").to_string()
    }
}

pub fn full_date(timestamp: i64) -> String {
    Local.timestamp_opt(timestamp, 0).single().map_or_else(String::new, |d| d.format("%a, %-d %b %Y, %H:%M").to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone};

    use super::*;

    #[test]
    fn initials_come_from_the_first_and_last_words() {
        assert_eq!(initials("Nguyễn Thị Minh Anh"), "NA");
        assert_eq!(initials("Linear"), "L");
        assert_eq!(initials("billing@cloud.example"), "B");
        assert_eq!(initials("[GitHub] - Notifications"), "GN");
        assert_eq!(initials("  "), "");
    }

    #[test]
    fn a_sender_keeps_one_colour() {
        assert_eq!(tone("a@example.com"), tone("a@example.com"));
        assert!(tone("a@example.com") >= 0);
    }

    #[test]
    fn dates_shorten_with_recency() {
        let zone = FixedOffset::east_opt(7 * 3600).unwrap();
        let now = zone.with_ymd_and_hms(2026, 11, 4, 15, 0, 0).unwrap();
        let at = |y, m, d, h| zone.with_ymd_and_hms(y, m, d, h, 5, 0).unwrap().timestamp();
        assert_eq!(short_date(at(2026, 11, 4, 9), now), "09:05");
        assert_eq!(short_date(at(2026, 11, 3, 23), now), "3 Nov");
        assert_eq!(short_date(at(2025, 12, 31, 9), now), "31 Dec 2025");
    }
}
