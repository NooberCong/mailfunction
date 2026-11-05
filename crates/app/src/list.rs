//! The message list as a Slint model. Rows come from the store in small pages as the list
//! scrolls them into view, so a mailbox of any size costs a few pages of memory.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Datelike, Local, TimeZone};
use mailfunction_core::store::{MessageRow, Reader};
use slint::{Model, ModelNotify, ModelTracker};

use crate::RowItem;

const PAGE: usize = 100;
const MAX_PAGES: usize = 8;

pub struct MessageList {
    reader: Rc<Reader>,
    label: Cell<i64>,
    count: Cell<usize>,
    pages: RefCell<HashMap<usize, Rc<Vec<MessageRow>>>>,
    notify: ModelNotify,
}

impl MessageList {
    pub fn new(reader: Rc<Reader>) -> Self {
        Self {
            reader,
            label: Cell::new(0),
            count: Cell::new(0),
            pages: RefCell::default(),
            notify: ModelNotify::default(),
        }
    }

    pub fn show_label(&self, label: i64) {
        self.label.set(label);
        self.refresh();
    }

    /// Picks up changes in the store.
    pub fn refresh(&self) {
        self.pages.borrow_mut().clear();
        self.count.set(self.reader.message_count(self.label.get()).unwrap_or(0));
        self.notify.reset();
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
