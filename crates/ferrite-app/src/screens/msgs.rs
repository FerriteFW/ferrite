//! APRS messages (MENU 0 5 2; manual chapter 25, APRS-6, APRS-8, APRS-13): the inbox and sent together,
//! newest first, sent ones with their state (C5); one message (C6); and compose (A16), typed with multi-tap
//! (KEY-12: digits, # case and digits, * space, BACK deletes). The draft lives with the APRS task's outgoing
//! message ([`crate::data::Draft`]), so typing costs the App no buffer. Nothing sends without a callsign
//! (APRS-8).

use super::lists::*;
use super::log::wrap;
use super::nav::Dest;
use super::opts::Of;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Draft, Message, MsgState, Op};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Msgs {
    pub cur: Cursor,
    /// The selected message's page (C6).
    pub open: bool,
}

const ALL: fn(u16) -> bool = |_| true;

/// `tries`: how often a message is tried in all now (APRS-13.6), for "try 2/5".
fn state_text(m: &Message, tries: u8, b: &mut Buf<16>) -> u16 {
    b.clear();
    match m.state {
        MsgState::Unread => {
            b.set(b"new");
            BLUE
        }
        MsgState::Read => SUB,
        MsgState::Pending => {
            b.set(b"pending");
            SUB
        }
        MsgState::Acked => {
            b.set(b"acked");
            GREEN
        }
        MsgState::Retrying(n) => {
            // APRS-13: the try it's on of its tries (APRS-13.6; "retry 2" read as a second retry on the first)
            let _ = uwrite!(b, "try {}/{}", n, tries.max(n));
            AMBER
        }
        MsgState::Failed => {
            b.set(b"failed");
            RED
        }
        MsgState::Rejected => {
            b.set(b"rejected");
            RED
        }
        MsgState::Cancelled => {
            b.set(b"cancelled");
            SUB
        }
    }
}

/// APRS-36: one of ours heard back, repeated by `by`: said once in a toast.
pub fn digipeated(app: &mut App, by: &[u8]) {
    let mut b = Buf::<16>::from(b"by ");
    b.push(by);
    app.show_toast(b"Digipeated", b.as_bytes(), false);
}

/// APRS-34: one of ours was rejected by `peer`: said in a toast (an ack only moves the state).
pub fn rejected(app: &mut App, peer: &[u8]) {
    app.show_toast(b"Message rejected", peer, false);
}

/// A draft addressed to `to` (empty: a new message), then compose.
fn compose(app: &mut App, store: &impl ChannelStore, to: &[u8]) {
    let d = store.data();
    let mut t = Buf::<9>::new();
    t.set(to);
    d.request(Op::Draft(Draft { to: t, ..d.draft() }));
    app.open(Dest::Compose);
}

/// The message selected under the options page: stop sending it (APRS-13.1) or send it again (APRS-13.2), and say
/// so. Sending again refuses as compose does (LINK-19, KISS-10, APRS-8, APRS-13.3).
pub(crate) fn stop_or_resend(app: &mut App, store: &impl ChannelStore, stop: bool) {
    let Screen::Msgs(s) = app.screen else { return };
    let data = store.data();
    let Some(m) = data.message(s.cur.sel) else { return };
    if stop {
        let ok = data.request(Op::StopMessage(s.cur.sel));
        return app.show_toast(if ok { b"Sending stopped" } else { b"Not sending" }, m.peer.as_bytes(), false);
    }
    if app.remote_tx_refused() {
        return;
    }
    if let Some(why) = app.kiss_holds_aprs() {
        return app.show_toast(b"Not sent", why.as_bytes(), false);
    }
    if !data.callsign().is_some_and(|c| crate::data::ax25_call_ok(c.as_bytes())) {
        return app.show_toast(b"Not sent", b"MENU 0 8 6 1", false); // APRS-8, SETUP-13.1
    }
    if !data.send_room() {
        return app.show_toast(b"Not sent", b"4 messages sending", false);
    }
    if data.request(Op::ResendMessage(s.cur.sel)) {
        // the new one is the newest: the list's first row
        app.screen = Screen::Msgs(Msgs { cur: Cursor::default(), ..s });
        app.show_toast(b"Message queued", m.peer.as_bytes(), false);
    } else {
        app.show_toast(b"Not sent", b"try again", false);
    }
}

/// APRS-46.1: the message under the list (or open) deleted; the list again, its cursor on the next one.
pub(crate) fn delete(app: &mut App, store: &impl ChannelStore) {
    let Screen::Msgs(s) = app.screen else { return };
    let data = store.data();
    let Some(m) = data.message(s.cur.sel) else { return };
    if !data.request(Op::DeleteMessage(s.cur.sel)) {
        return app.show_toast(b"Not deleted", b"try again", false);
    }
    if m.inbound && m.state == MsgState::Unread {
        app.inbox = app.inbox.saturating_sub(1); // INBOX-3.1: one unread fewer
    }
    let n = data.messages();
    let mut cur = s.cur;
    cur.sel = cur.sel.min(n.saturating_sub(1));
    cur.top = cur.top.min(cur.sel);
    app.screen = Screen::Msgs(Msgs { cur, open: false });
    app.show_toast(b"Message deleted", m.peer.as_bytes(), false);
}

impl View for Msgs {
    /// Hold MENU: the messages' options, with Stop sending or Send again first on one of ours (APRS-13.1, 13.2).
    fn own_hold_menu(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        if self.open { "APRS message" } else { "APRS messages" }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.open { b"Message" } else { b"Messages" });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        let m = d.message(self.cur.sel);
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Long) => {
                let of = match m.map(|m| (m.inbound, m.state)) {
                    Some((false, MsgState::Pending | MsgState::Retrying(_))) => Of::MsgSending,
                    Some((false, MsgState::Failed | MsgState::Cancelled)) => Of::MsgEnded,
                    Some(_) => Of::Msg,
                    None => Of::Messages,
                };
                app.open_screen(of.screen());
            }
            (Key::Menu, Short) if !self.open && m.is_some() => {
                let unread = m.is_some_and(|m| m.inbound && m.state == MsgState::Unread);
                if d.request(Op::MessageRead(self.cur.sel)) && unread {
                    // INBOX-3.1: one message fewer; INBOX-7.1: its log entry read with it.
                    (app.inbox, app.log_unread) = (app.inbox.saturating_sub(1), app.log_unread.saturating_sub(1));
                }
                app.screen = Screen::Msgs(Msgs { open: true, ..self });
            }
            (Key::Menu, Short) => {
                if let Some(m) = m {
                    compose(app, store, m.peer.as_bytes()); // reply
                }
            }
            (Key::Hash, Short) if !self.open => compose(app, store, b""),
            (Key::Exit, Short) if self.open => app.screen = Screen::Msgs(Msgs { open: false, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let cur = self.cur.step(d, store.data().messages(), ROWS2, &ALL);
        if self.open && cur.sel != self.cur.sel {
            let unread = store.data().message(cur.sel).is_some_and(|m| m.inbound && m.state == MsgState::Unread);
            if store.data().request(Op::MessageRead(cur.sel)) && unread {
                (app.inbox, app.log_unread) = (app.inbox.saturating_sub(1), app.log_unread.saturating_sub(1)); // INBOX-3.1, INBOX-7.1
            }
        }
        app.screen = Screen::Msgs(Msgs { cur, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, self.open);
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        let tries = app.aprs_cfg().tries();
        let row = |m: &Message, time: &mut Buf<12>, st: &mut Buf<16>| -> u16 {
            clock_text(cx.dev.clock_min, m.ago_s, time);
            state_text(m, tries, st)
        };
        if self.open {
            if data.message(self.cur.sel).is_none() {
                app.empty(lcd, b"No such message", b"");
            }
            if let Some(m) = data.message(self.cur.sel) {
                let (mut time, mut st, mut age) = (Buf::<12>::new(), Buf::<16>::new(), Buf::<12>::new());
                let c = row(&m, &mut time, &mut st);
                age_text(m.ago_s, &mut age);
                let top = Row2 { tag: if m.inbound { b"From" } else { b"To" }, title: m.peer.as_bytes(), big: true, right: time.as_bytes(), sub: if m.inbound { b"Received" } else { b"Sent" }, ..Row2::default() };
                if app.dirty(R_TOP, Mix::new().any(&top).get()) {
                    row2(lcd, 22, &top, w::Cue::default());
                }
                let mut b = Buf::<40>::new();
                // The clock time is in the header: here how long ago, and for a sent message its state.
                let y = app.kv(lcd, 0, 58, 0, &w::KV_DETAIL, b"Age", if m.ago_s == crate::data::AGO_UNKNOWN { &b"Before restart"[..] } else { ferrite_ui::v!(b, "{} ago", Txt(age.as_bytes())) }, TEXT, false);
                let (l, v): (&[u8], &[u8]) = if m.inbound { (b"", b"") } else { (b"State", st.as_bytes()) };
                app.kv(lcd, 1, y, 0, &w::KV_DETAIL, l, v, c, false);
                if app.dirty(R_NOTE, Mix::new().h(&m.text).get()) {
                    ferrite_ui::canvas::banded(lcd, 0, 100, 240, 196, BG, |b| {
                        wrap(b, 8, 104, 224, 296, m.text.as_bytes(), TEXT);
                    });
                }
            }
            return;
        }
        let n = data.messages();
        let new = (0..n).filter(|&i| data.message(i).is_some_and(|m| m.state == MsgState::Unread)).count();
        let mut l = Buf::<24>::new();
        let _ = uwrite!(l, "{} {}", n, if n == 1 { "message" } else { "messages" });
        if new > 0 {
            let _ = uwrite!(l, " \u{b7} {} new", new);
        }
        app.list_head(lcd, l.as_bytes(), b"");
        let rows = self.cur.rows(n, ROWS2, &ALL, |r, i| {
            if let Some(m) = data.message(i) {
                let (mut time, mut st) = (Buf::<12>::new(), Buf::<16>::new());
                let c = row(&m, &mut time, &mut st);
                // APRS-43: a saved contact's name leads the line
                let mut sub = Buf::<84>::new();
                if let Some(k) = cx.store.data().contact_of(m.peer.as_bytes()) {
                    sub.push(k.name.as_bytes());
                    sub.push(" \u{b7} ".as_bytes());
                }
                sub.push(m.text.as_bytes());
                let r2 = Row2 {
                    tag: if m.inbound { b"From" } else { b"To" },
                    title: m.peer.as_bytes(),
                    big: true,
                    right: time.as_bytes(),
                    sub: sub.as_bytes(),
                    sub_right: st.as_bytes(),
                    sub_c: if matches!(m.state, MsgState::Failed | MsgState::Rejected) { c } else { 0 },
                    dot: m.state == MsgState::Unread,
                    selected: i == self.cur.sel,
                    ..Row2::default()
                };
                app.row2(lcd, r, &r2);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &ALL);
        if n == 0 {
            app.empty(lcd, b"No messages", b"# writes one");
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.open {
            Hints::new(&[("OK", "Reply"), (ARROWS_UD, "Next"), ("BACK", "List")])
        } else {
            Hints::new(&[("OK", "Open"), ("#", "New"), ("Hold MENU", "More")])
        }
    }
}

// ---- compose (A16) ----

/// Multi-tap letters per digit (KEY-12); 1 has the punctuation, the same marks as predictive's (TEXT-2.1).
const TAPS: [&[u8]; 10] = [b" 0", b".,?!'/-@:+1", b"abc2", b"def3", b"ghi4", b"jkl5", b"mno6", b"pqrs7", b"tuv8", b"wxyz9"];
/// The same key again within this many ms changes the last letter instead of adding one.
pub const TAP_MS: u32 = 1000;
/// APRS limits: an addressee of 9, a message of 67 characters.
const TO_MAX: usize = 9;
const TEXT_MAX: usize = 67;
/// Predictive: what 1 cycles through (TEXT-2.1). Printable ASCII, none of APRS's | ~ { (TEXT-13).
const PUNCT: &[u8] = b".,?!'/-@:+";
/// Compose's text is the draft's, unless a quick message is being edited (TEXT-7) or the comment (APRS-50).
const DRAFT: u8 = 255;
const COMMENT: u8 = 254;
/// APRS-57: auto reply's text (0 5 9 9).
const REPLY: u8 = 253;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Compose {
    /// 0 the recipient, 1 the text.
    pub field: u8,
    /// Multi-tap: 0 lower case, 1 capitals, 2 digits (# cycles).
    pub case: u8,
    /// The last multi-tap key (its digit + 1, 0 none), how often it was tapped, and when (ms / 64).
    pub key: u8,
    pub taps: u8,
    pub at: u16,
    /// Predictive (TEXT-2): the candidate picked for the keys typed (the matches best first, each followed by
    /// its short form), the case of the next word (0 Capital, 1 CAPS, 2 lower; set by what comes before it), the punctuation mark + 1 when 1 was
    /// the last key, and the quick message highlighted on an empty message (TEXT-6). The keys typed for the
    /// word in progress are the App's text buffer, as digits.
    pub pick: u8,
    pub pcase: u8,
    pub punct: u8,
    pub quick: u8,
    /// [`DRAFT`], the quick message being edited (TEXT-7), or [`COMMENT`] (APRS-50). `quick` is also the address book's row while the
    /// recipient is empty (APRS-38): the two lists are never up together.
    pub edit: u8,
    /// [`LISTED`]: the quick list, or the address book (APRS-38), is on screen; [`SVC`]: the recipient is a
    /// service (APRS-39: its format shown, multi-tap whatever TEXT-10's choice). Set when resolved and drawn: the
    /// hint bar has no store to ask.
    pub list: u8,
    /// The cursor (TEXT-12): how many characters of the message are after it; 0 at the end. Kept as the count
    /// after it, it stays put while the text before it changes.
    pub tail: u8,
}

const LISTED: u8 = 1;
const SVC: u8 = 2;
/// The quick list put away (TEXT-6.1): * ("Write") or any key that types; the knob on an empty message brings it back.
const WRITE: u8 = 4;

/// Not decided yet: the first key or draw puts the cursor on the text when the draft has a recipient.
const UNSET: u8 = 255;

impl Compose {
    pub fn new() -> Self {
        Compose { field: UNSET, case: 0, key: 0, taps: 0, at: 0, pick: 0, pcase: 0, punct: 0, quick: 0, edit: DRAFT, list: 0, tail: 0 }
    }

    /// Quick message `i` in the editor (TEXT-7).
    pub fn quick_edit(i: u8) -> Self {
        Compose { field: 1, edit: i, ..Compose::new() }
    }

    fn resolved(self, store: &impl ChannelStore) -> Self {
        let d = store.data().draft();
        let svc = !self.quick_edit_on() && super::contacts::service(d.to.as_bytes()).is_some();
        let list = self.list & (LISTED | WRITE) | if svc { SVC } else { 0 };
        if self.field != UNSET {
            return Compose { list, ..self };
        }
        let to_set = !d.to.is_empty();
        // a message already begun goes on in lower case; a new one (or after . ? !) starts with a capital
        Compose { field: to_set as u8, case: if to_set { 0 } else { 1 }, pcase: next_case(d.text.as_bytes(), 2), list, ..self }
    }

    /// Predictive entry for this message: TEXT-10's choice, but never to a service (APRS-39).
    fn pred(self, app: &App) -> bool {
        app.set.predictive && self.list & SVC == 0
    }

    /// The address book is up (APRS-38): an empty recipient being typed.
    pub(crate) fn book_up(self, store: &impl ChannelStore) -> bool {
        self.field == 0 && !self.quick_edit_on() && store.data().draft().to.is_empty()
    }

    /// The editor holds a quick message or the comment (APRS-50), not the draft: no recipient, OK keeps it.
    pub(crate) fn quick_edit_on(self) -> bool {
        self.edit != DRAFT
    }

    /// 0 5 5 3's comment in the editor (APRS-50).
    pub fn comment_edit() -> Self {
        Compose { field: 1, edit: COMMENT, ..Compose::new() }
    }

    fn comment_on(self) -> bool {
        self.edit == COMMENT
    }

    /// APRS-57: auto reply's text in the editor (0 5 9 9).
    pub fn reply_edit() -> Self {
        Compose { field: 1, edit: REPLY, ..Compose::new() }
    }

    fn reply_on(self) -> bool {
        self.edit == REPLY
    }

    /// The text's limit: 67 characters (TEXT-5), the comment's 40 (APRS-50).
    fn max(self) -> usize {
        if self.comment_on() {
            crate::aprs::COMMENT_LEN
        } else if self.reply_on() {
            crate::aprs::REPLY_TEXT_LEN
        } else {
            TEXT_MAX
        }
    }
}

impl Compose {
    /// What compose shows, for the UI readback (LINK-19): the recipient, the count against the 67 (TEXT-5), the
    /// message as drawn (the word being predicted in it), and which field has the cursor (0 to, 1 text).
    pub(crate) fn shown(self, app: &App, store: &impl ChannelStore) -> (Buf<16>, Buf<12>, Buf<84>, u8) {
        let me = self.resolved(store);
        let d = store.data().draft();
        let keys = app.text;
        let head = text_of(app, store, me);
        let full = full_of(app, store, me);
        let (word, _, drop) = if me.pred(app) && !keys.is_empty() { pending(keys.as_bytes(), head.as_bytes(), me) } else { (Buf::new(), true, 0) };
        let mut c = Buf::<12>::new();
        let _ = uwrite!(c, "{}/{}", full.len() - drop + word.len(), me.max());
        let mut all = Buf::<84>::from(&head.as_bytes()[..head.len() - drop]);
        all.push(word.as_bytes());
        all.push(&full.as_bytes()[head.len().min(full.len())..]);
        let to = if me.quick_edit_on() { Buf::new() } else { Buf::from(d.to.as_bytes()) };
        (to, c, all, me.field)
    }
}

/// Quick message `i` (TEXT-7): the owner's, kept with the APRS store, or the factory text.
pub(crate) fn quick_text(store: &impl ChannelStore, i: usize) -> Buf<67> {
    store.data().quick(i).unwrap_or_else(|| Buf::from(crate::words::quick(i)))
}

/// The text being composed: the draft's, the quick message being edited, or the comment (APRS-50).
fn full_of(app: &App, store: &impl ChannelStore, s: Compose) -> Buf<67> {
    if s.comment_on() {
        Buf::from(app.aprs_cfg().comment_text())
    } else if s.reply_on() {
        Buf::from(app.aprs_cfg().reply_text())
    } else if s.quick_edit_on() {
        quick_text(store, s.edit as usize)
    } else {
        store.data().draft().text
    }
}

/// The text before the cursor (TEXT-12): what typing and deleting work on.
fn text_of(app: &App, store: &impl ChannelStore, s: Compose) -> Buf<67> {
    let t = full_of(app, store, s);
    Buf::from(&t.as_bytes()[..t.len() - (s.tail as usize).min(t.len())])
}

/// The text before the cursor back in, with what was after the cursor after it.
fn put_text(app: &mut App, store: &impl ChannelStore, s: Compose, head: Buf<67>) {
    let full = full_of(app, store, s);
    let mut t = head;
    t.push(&full.as_bytes()[full.len() - (s.tail as usize).min(full.len())..]);
    let data = store.data();
    if s.comment_on() {
        app.set_aprs_comment(t.as_bytes());
    } else if s.reply_on() {
        app.set_aprs_reply_text(t.as_bytes());
    } else if s.quick_edit_on() {
        data.request(Op::Quick(s.edit, Some(t)));
    } else {
        data.request(Op::Draft(Draft { text: t, ..data.draft() }));
    }
}

/// A candidate: the word, whether it is shown in capitals, and how many bytes before it a phrase's short form
/// replaces (0 for a word).
type Cand = (Buf<16>, bool, usize);

/// Candidate `i` for `keys` after `head` (TEXT-2, TEXT-4): the dictionary's matches best first, each followed by
/// its short form, and by a phrase's when the words before it in `head` complete one (see you + later → CUL).
fn candidate(keys: &[u8], head: &[u8], i: usize) -> Option<Cand> {
    let mut k = 0;
    for n in 0.. {
        let w = crate::words::nth(keys, n)?;
        let short = crate::words::short(w.text.as_bytes()).map(|s| (s, 0));
        let phrase = crate::words::phrase(head, w.text.as_bytes());
        for c in [Some((w.text, w.caps, 0)), short.map(|(s, d)| (Buf::from(s), true, d)), phrase.map(|(s, d)| (Buf::from(s), true, d))].into_iter().flatten() {
            if k == i {
                return Some(c);
            }
            k += 1;
        }
    }
    None
}

fn candidates(keys: &[u8], head: &[u8]) -> usize {
    (0..).take_while(|&i| candidate(keys, head, i).is_some()).count()
}

/// A word as it goes in: capitals for Q-codes and shorthand, else the case chosen (Capital, CAPS, lower).
fn cased(w: &[u8], caps: bool, pcase: u8) -> Buf<16> {
    let mut b = Buf::<16>::from(w);
    let mut t = [0u8; 16];
    for (i, (o, &c)) in t.iter_mut().zip(b.as_bytes()).enumerate() {
        *o = if caps || pcase == 1 || (pcase == 0 && i == 0) { c.to_ascii_uppercase() } else { c };
    }
    let n = b.len();
    b.set(&t[..n]);
    b
}

/// What the word in progress shows as: the picked candidate, or the keys as digits when nothing matches; and
/// how many bytes of `head` it replaces (a phrase's short form).
fn pending(keys: &[u8], head: &[u8], s: Compose) -> (Buf<16>, bool, usize) {
    match candidate(keys, head, s.pick as usize) {
        Some((w, caps, drop)) => (cased(w.as_bytes(), caps, s.pcase), true, drop),
        None => (Buf::from(keys), false, 0),
    }
}

/// After a word or a mark: the next word's case. Capital starts a message and follows . ? !; CAPS stays.
fn next_case(t: &[u8], pcase: u8) -> u8 {
    let end = t.iter().rev().find(|&&c| c != b' ');
    match (pcase, end) {
        (1, _) => 1,
        (_, None | Some(b'.' | b'?' | b'!')) => 0,
        _ => 2,
    }
}

/// The 67-character limit (TEXT-5), or the comment's 40 (APRS-50): a short beep, and why in a toast.
fn refuse_full(app: &mut App, s: Compose) {
    app.beep_refused();
    if s.comment_on() {
        app.show_toast("40 characters".as_bytes(), "the comment limit".as_bytes(), false);
    } else if s.reply_on() {
        app.show_toast("45 characters".as_bytes(), "the reply limit".as_bytes(), false);
    } else {
        app.show_toast("67 characters".as_bytes(), "the APRS limit".as_bytes(), false);
    }
}

/// Room before the cursor: the 67 characters (the comment's 40) less what is after it.
fn room(s: Compose) -> usize {
    s.max().saturating_sub(s.tail as usize)
}

/// The word in progress goes into the text (no space); false when it can't (no word, or no room).
fn accept(app: &mut App, s: &mut Compose, t: &mut Buf<67>) -> bool {
    if app.text.is_empty() {
        return true;
    }
    let keys = app.text;
    let (w, ok, drop) = pending(keys.as_bytes(), t.as_bytes(), *s);
    if !ok {
        app.show_toast(b"No word", "# to spell it".as_bytes(), false); // TEXT-3
        return false;
    }
    if t.len() - drop + w.len() > room(*s) {
        refuse_full(app, *s);
        return false;
    }
    let head = *t;
    t.set(&head.as_bytes()[..head.len() - drop]);
    t.push(w.as_bytes());
    app.text.clear();
    s.pick = 0;
    s.pcase = next_case(t.as_bytes(), if s.pcase == 0 { 2 } else { s.pcase });
    true
}

impl Compose {
    /// Predictive keys on the message text (TEXT-2 to TEXT-6). True when the key was taken.
    fn pred_key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) -> bool {
        use Press::*;
        let mut s = self;
        let mut t = text_of(app, store, s);
        let keys = app.text;
        let quick_list = t.is_empty() && s.tail == 0 && keys.is_empty() && !s.quick_edit_on() && s.list & WRITE == 0;
        match (e.key, e.press) {
            (Key::Digit(d @ 2..=9), Short) => {
                if t.len() + keys.len() + 1 > room(s) || keys.len() >= crate::words::WORD_MAX {
                    refuse_full(app, s);
                    return true;
                }
                app.text.push(&[b'0' + d]);
                s.pick = 0;
                s.punct = 0;
            }
            // hold a digit: the digit itself (73, 146.52)
            (Key::Digit(d), Long) => {
                if !accept(app, &mut s, &mut t) {
                    return true;
                }
                if t.len() + 1 > room(s) {
                    refuse_full(app, s);
                    return true;
                }
                t.push(&[b'0' + d]);
                s.punct = 0;
            }
            // 1: punctuation after a word, again for the next mark
            (Key::Digit(1), Short) => {
                if s.punct > 0 && keys.is_empty() {
                    let i = s.punct as usize % PUNCT.len();
                    pop(&mut t);
                    t.push(&PUNCT[i..i + 1]);
                    s.punct = i as u8 + 1;
                } else {
                    if !accept(app, &mut s, &mut t) {
                        return true;
                    }
                    if t.len() + 1 > room(s) {
                        refuse_full(app, s);
                        return true;
                    }
                    t.push(&PUNCT[..1]);
                    s.punct = 1;
                }
                s.pcase = next_case(t.as_bytes(), s.pcase);
            }
            // * (marked _, and 0): the word, then a space (KEY-12)
            // TEXT-6.1: * on the quick list puts it away for an empty editor
            (Key::Star, Short) if quick_list => {}
            (Key::Star | Key::Digit(0), Short) => {
                if !accept(app, &mut s, &mut t) {
                    return true;
                }
                if !t.is_empty() && t.len() < room(s) {
                    t.push(b" ");
                }
                s.punct = 0;
                s.pcase = next_case(t.as_bytes(), s.pcase);
            }
            // TEXT-10: # (marked ^) goes on to multi-tap's abc, the word in progress taken first; T9's case stays
            // automatic (a capital starts a message and follows . ? !, Q-codes in capitals)
            (Key::Hash, Short) => {
                // keys that spell nothing go (TEXT-3: # to spell it), as does a word with no room
                if pending(keys.as_bytes(), t.as_bytes(), s).1 && !accept(app, &mut s, &mut t) {
                    app.text.clear();
                }
                app.text.clear();
                app.set.predictive = false;
                (s.case, s.key, s.pick, s.punct) = (0, 0, 0, 0);
            }
            (Key::Up | Key::Down, Short | Repeat) if !keys.is_empty() || quick_list => {
                let d = if e.key == Key::Up { -1 } else { 1 };
                return self.pred_knob(app, d, store);
            }
            (Key::Exit, Short) if !keys.is_empty() => {
                let mut k = keys;
                pop(&mut k);
                app.text = k;
                s.pick = 0;
            }
            (Key::Menu, Short) if quick_list => {
                // TEXT-6: the quick message goes in; OK again sends
                t = quick_text(store, s.quick as usize);
                s.pcase = next_case(t.as_bytes(), 2);
            }
            (Key::Menu, Short) if !keys.is_empty() => {
                if !accept(app, &mut s, &mut t) {
                    return true;
                }
                put_text(app, store, s, t);
                app.screen = Screen::Compose(s);
                return false; // OK goes on to send (or keep the quick message)
            }
            _ => return false,
        }
        // TEXT-6.1: whatever typed (or * on the list) puts the quick list away until the knob brings it back
        s.list |= WRITE;
        put_text(app, store, s, t);
        app.screen = Screen::Compose(s);
        true
    }

    /// The knob or ▲▼ in predictive entry: through the candidates, or the quick list (TEXT-2, TEXT-6).
    fn pred_knob(self, app: &mut App, d: i32, store: &impl ChannelStore) -> bool {
        let keys = app.text;
        if !keys.is_empty() {
            let n = candidates(keys.as_bytes(), text_of(app, store, self).as_bytes()).max(1) as i32;
            let pick = (self.pick as i32 + d).rem_euclid(n) as u8;
            app.screen = Screen::Compose(Compose { pick, ..self });
            return true;
        }
        if full_of(app, store, self).is_empty() && !self.quick_edit_on() {
            if self.list & WRITE != 0 {
                // TEXT-6.1: the quick list back on an empty message
                app.screen = Screen::Compose(Compose { list: self.list & !WRITE, tail: 0, ..self });
                return true;
            }
            let quick = (self.quick as i32 + d).rem_euclid(crate::words::QUICK as i32) as u8;
            app.screen = Screen::Compose(Compose { quick, ..self });
            return true;
        }
        false
    }
}

/// OK on the text, or PTT in compose (APRS-18): the message goes to the APRS task's queue, a word being
/// predicted taken as it stands.
pub(crate) fn send(app: &mut App, store: &impl ChannelStore) {
    let Screen::Compose(c) = app.screen else { return };
    if app.remote_tx_refused() {
        return; // LINK-19: a message is a transmission
    }
    if let Some(why) = app.kiss_holds_aprs() {
        return app.show_toast(b"Not sent", why.as_bytes(), false); // KISS-10
    }
    let mut s = c.resolved(store);
    if s.field == 1 && s.pred(app) && !app.text.is_empty() {
        let mut t = text_of(app, store, s);
        if !accept(app, &mut s, &mut t) {
            return; // it said why
        }
        put_text(app, store, s, t);
        app.screen = Screen::Compose(Compose { key: 0, pick: 0, punct: 0, ..s });
    }
    let data = store.data();
    let d = data.draft();
    // APRS-8: nothing sends without a callsign.
    if data.callsign().is_none() {
        return app.show_toast("No callsign".as_bytes(), "MENU 0 8 6 1".as_bytes(), false);
    }
    if !data.callsign().is_some_and(|c| crate::data::ax25_call_ok(c.as_bytes())) {
        return app.show_toast("Callsign over 6".as_bytes(), "no APRS".as_bytes(), false); // SETUP-13.1
    }
    let text = full_of(app, store, s);
    if d.to.is_empty() || text.is_empty() {
        return app.show_toast("Nothing to send".as_bytes(), if d.to.is_empty() { "add a recipient".as_bytes() } else { "write a message".as_bytes() }, false);
    }
    if !data.send_room() {
        // APRS-13.3: four being tried already; the draft stays for later
        return app.show_toast(b"Not sent", b"4 messages sending", false);
    }
    if data.request(Op::Send) {
        app.back();
        app.show_toast("Message queued".as_bytes(), d.to.as_bytes(), false);
    } else {
        app.show_toast("Not sent".as_bytes(), "try again".as_bytes(), false);
    }
}

impl View for Compose {
    fn name(self) -> &'static str {
        if self.comment_on() {
            "Comment"
        } else if self.reply_on() {
            "Auto reply text"
        } else if self.quick_edit_on() {
            "Quick message"
        } else {
            "Compose message"
        }
    }

    /// ◀▶ move the cursor in the message (TEXT-12); ▲▼ and the knob stay on the words.
    fn own_arrows(self) -> bool {
        true
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.comment_on() {
            b"Comment"
        } else if self.reply_on() {
            b"Reply"
        } else if self.quick_edit_on() {
            b"Quick"
        } else {
            b"Compose"
        });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self.resolved(store);
        // APRS-38: the address book on an empty recipient.
        if s.book_up(store) {
            let d = store.data();
            let b = super::contacts::Book::new(d);
            let n = b.len();
            let at = (s.quick as u16).min(n - 1);
            let book = match (e.key, e.press) {
                (Key::Up | Key::Down, Short | Repeat) => Some((at as i32 + if e.key == Key::Up { -1 } else { 1 }).rem_euclid(n as i32) as u16),
                (Key::Left | Key::Right, Short | Repeat) => Some(b.jump(at, if e.key == Key::Left { -1 } else { 1 })),
                (Key::Menu, Short) => {
                    if let Some((_, call, _)) = b.row(d, at) {
                        let mut to = Buf::<9>::new();
                        to.set(call.as_bytes());
                        d.request(Op::Draft(Draft { to, ..d.draft() }));
                        app.screen = Screen::Compose(Compose { field: 1, case: 0, key: 0, tail: 0, quick: 0, ..s });
                    }
                    return;
                }
                _ => None,
            };
            if let Some(k) = book {
                app.screen = Screen::Compose(Compose { quick: k as u8, ..s });
                return;
            }
        }
        // TEXT-12: ◀▶ move the cursor in the message; a word being predicted goes in first.
        if s.field == 1 && matches!((e.key, e.press), (Key::Left | Key::Right, Short | Repeat)) {
            let mut t = text_of(app, store, s);
            if s.pred(app) && !accept(app, &mut s, &mut t) {
                return;
            }
            put_text(app, store, s, t);
            let n = full_of(app, store, s).len() as u8;
            s.tail = if e.key == Key::Left { (s.tail + 1).min(n) } else { s.tail.saturating_sub(1) };
            app.screen = Screen::Compose(Compose { key: 0, pick: 0, punct: 0, ..s });
            return;
        }
        if s.field == 1 && s.pred(app) && s.pred_key(app, e, store) {
            return;
        }
        if let Screen::Compose(c) = app.screen {
            s = c.resolved(store); // pred_key may have stored the accepted word
        }
        let data = store.data();
        let mut d = data.draft();
        let now = (app.now / 64) as u16;
        let to = s.field == 0;
        // the field being typed in, as one buffer
        let mut cur: Buf<67> = if to { Buf::from(d.to.as_bytes()) } else { text_of(app, store, s) };
        let max = if to { TO_MAX } else { room(s) };
        s.punct = 0; // 1 starts a new mark after anything else (a BACK, too)
        match (e.key, e.press) {
            (Key::Digit(k), Short) => {
                let set = TAPS[k as usize];
                let ch = |taps: u8| {
                    let c = if s.case == 2 { b'0' + k } else { set[taps as usize % set.len()] };
                    if s.case == 1 || to { c.to_ascii_uppercase() } else { c }
                };
                // the recipient takes capitals, digits and "-" only
                let ok = |c: u8| !to || c.is_ascii_alphanumeric() || c == b'-';
                let again = s.key == k + 1 && now.wrapping_sub(s.at) < (TAP_MS / 64) as u16 && s.case != 2 && !cur.is_empty();
                s.taps = if again { s.taps.wrapping_add(1) } else { 0 };
                while !ok(ch(s.taps)) {
                    s.taps = s.taps.wrapping_add(1);
                }
                if again {
                    pop(&mut cur);
                    cur.push(&[ch(s.taps)]);
                } else if cur.len() < max {
                    cur.push(&[ch(s.taps)]);
                } else if !to {
                    refuse_full(app, s);
                }
                s.key = k + 1;
                s.at = now;
            }
            // APRS-17.8: V/M sends a beacon; the draft stays as it is
            (Key::VfoMem, Short) => return super::opts::beacon_now(app, store),
            // TEXT-10: # goes round abc, ABC, 123, then T9 on the message (never to a service, APRS-39: back to abc)
            (Key::Hash, Short) if !to && s.case == 2 && s.list & SVC == 0 => {
                put_text(app, store, s, cur);
                app.set.predictive = true;
                app.text.clear();
                let t = text_of(app, store, s);
                app.screen = Screen::Compose(Compose { case: 0, key: 0, pick: 0, punct: 0, pcase: next_case(t.as_bytes(), 2), ..s });
                return;
            }
            (Key::Hash, Short) => {
                s.case = (s.case + 1) % 3;
                s.key = 0;
            }
            (Key::Star, Short) if !to && cur.len() < max => {
                cur.push(b" ");
                s.key = 0;
            }
            (Key::Exit, Short) => {
                if !cur.is_empty() {
                    pop(&mut cur);
                } else if !to && s.tail > 0 {
                    // at the start with text after the cursor: nothing before it to delete
                } else if to || s.quick_edit_on() {
                    return app.back(); // the draft stays with the APRS task
                } else {
                    s.field = 0;
                    s.case = 1;
                }
                s.key = 0;
            }
            (Key::Down | Key::Up, Short) if !s.quick_edit_on() => {
                s.field ^= 1;
                (s.key, s.tail) = (0, 0);
                s.case = if s.field == 0 { 1 } else { 0 };
            }
            (Key::Menu, Short) if to => {
                s.field = 1;
                (s.key, s.tail) = (0, 0);
                s.case = 0;
            }
            // APRS-50: the comment as typed is kept; an empty one is none
            (Key::Menu, Short) if s.comment_on() => {
                app.back();
                let gone = app.aprs_cfg().comment_text().is_empty();
                return app.show_toast("Comment".as_bytes(), if gone { b"cleared" } else { b"kept" }, false);
            }
            // APRS-57: the reply as typed; an empty one sends none
            (Key::Menu, Short) if s.reply_on() => {
                app.back();
                let gone = app.aprs_cfg().reply_text().is_empty();
                return app.show_toast("Auto reply".as_bytes(), if gone { b"cleared" } else { b"kept" }, false);
            }
            (Key::Menu, Short) if s.quick_edit_on() => {
                app.back();
                return app.show_toast("Quick message".as_bytes(), b"kept", false);
            }
            (Key::Menu, Short) => return send(app, store),
            _ => return,
        }
        if to {
            d.to.set(cur.as_bytes());
            data.request(Op::Draft(d));
        } else {
            put_text(app, store, s, cur);
        }
        app.screen = Screen::Compose(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let s = self.resolved(store);
        if s.book_up(store) {
            let n = super::contacts::Book::new(store.data()).len() as i32;
            app.screen = Screen::Compose(Compose { quick: (s.quick as i32 + d).rem_euclid(n) as u8, ..s });
        } else if s.field == 1 && s.pred(app) {
            s.pred_knob(app, d, store);
        }
    }

    fn tick(self, app: &mut App, _: &LiveAll) {
        // The pending letter is kept once the tap time has passed: its underline goes.
        if self.key != 0 && ((app.now / 64) as u16).wrapping_sub(self.at) >= (TAP_MS / 64) as u16 {
            app.screen = Screen::Compose(Compose { key: 0, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let d = cx.store.data().draft();
        let me = self.resolved(cx.store);
        if me != self {
            app.screen = Screen::Compose(me);
            return me.render(app, lcd, cx);
        }
        app.page(lcd, 0u8);
        let pred = self.pred(app);
        let keys = app.text;
        let head = text_of(app, cx.store, self);
        let full = full_of(app, cx.store, self);
        let (word, matched, drop) = if pred && !keys.is_empty() { pending(keys.as_bytes(), head.as_bytes(), self) } else { (Buf::new(), true, 0) };
        // a phrase's short form shows in place of the words it replaces (TEXT-4)
        let text = Buf::<67>::from(&head.as_bytes()[..head.len() - drop]);
        let quick_list = pred && self.field == 1 && full.is_empty() && keys.is_empty() && !self.quick_edit_on() && self.list & WRITE == 0;
        let listed = quick_list || self.book_up(cx.store);
        if listed != (self.list & LISTED != 0) {
            app.screen = Screen::Compose(Compose { list: self.list & (SVC | WRITE) | listed as u8, ..self });
        }
        let pending_tap = self.key != 0;
        // the recipient, or which quick message this is
        if app.dirty(R_TOP, Mix::new().h(&d.to).h(&self.field).h(&(pending_tap && self.field == 0)).h(&self.edit).get()) {
            if self.quick_edit_on() {
                ferrite_ui::canvas::banded(lcd, 0, 24, 240, 60, BG, |lcd| {
                    let mut b = Buf::<40>::new();
                    if self.comment_on() {
                        b.push(b"Beacon comment"); // APRS-50
                    } else if self.reply_on() {
                        b.push(b"Auto reply"); // APRS-57
                    } else {
                        let _ = uwrite!(b, "Quick message {}", self.edit + 1);
                    }
                    lcd.text4(8, 36, b.as_bytes(), &UI_12B, TEXT, BG);
                    let sub = if self.comment_on() {
                        "After every position report"
                    } else if self.reply_on() {
                        "Sent to who messages you"
                    } else {
                        "# in the list: back to factory"
                    };
                    lcd.text4(8, 56, sub.as_bytes(), &UI_12, SUB, BG);
                });
            } else {
                field_box(lcd, 24, 34, b"To", d.to.as_bytes(), self.field == 0, pending_tap && self.field == 0, "callsign-SSID".as_bytes());
            }
        }
        let (y, h) = (86, 120);
        let ver = cx.store.data().version();
        if self.book_up(cx.store) {
            // APRS-38, DISP-28: the book is built only when the pick or the data changed, and only the rows that
            // changed repaint (it was rebuilt from flash and its whole box repainted on every pass and step).
            if app.dirty(R_LIST_GATE, Mix::new().h(&1u8).h(&self.quick).h(&ver).get()) {
                let b = super::contacts::Book::new(cx.store.data());
                book_rows(app, lcd, cx.store.data(), &b, self.quick as u16, y, h, ver);
            }
        } else {
            app.forget(R_LIST_GATE, 1); // the book draws whole when it comes back
            let focus = self.field == 1;
            let bg = if focus { focus_fill() } else { SURFACE };
            let key = Mix::new().h(&full).h(&self.tail).h(&word).h(&self.field).h(&(pending_tap && self.field == 1)).h(&quick_list).get();
            // DISP-28: the quick list's box once; its rows below, each when it changes.
            let boxed = app.dirty(R_LIST_BOX, if quick_list { Mix::new().h(&2u8).h(&self.field).get() } else { Mix::new().h(&9u8).h(&key).get() });
            let full_box = boxed || !quick_list;
            if app.dirty(R_ROW, key) || boxed {
                ferrite_ui::canvas::banded(lcd, 0, y, 240, if full_box { h + 24 } else { 18 }, BG, |lcd| {
                    let label = if quick_list {
                        "Quick messages".as_bytes()
                    } else if self.comment_on() {
                        b"Comment".as_slice()
                    } else {
                        b"Message".as_slice()
                    };
                    lcd.text4(8, y + 12, label, &UI_12, SUB, BG);
                    let mut c = Buf::<12>::new();
                    let _ = uwrite!(c, "{}/{}", full.len() - drop + word.len(), self.max()); // TEXT-5, APRS-50
                    let cw = text4_width(c.as_bytes(), &UI_12);
                    lcd.text4(232 - cw, y + 12, c.as_bytes(), &UI_12, SUB, BG);
                    if !full_box {
                        return;
                    }
                    lcd.fill_r(8, y + 18, 224, h, 2, bg);
                    lcd.frame_r(8, y + 18, 224, h, 2, if focus { BLUE } else { LINE });
                    if !quick_list {
                        let mut all = Buf::<84>::from(text.as_bytes());
                        all.push(word.as_bytes());
                        let at = all.len();
                        all.push(&full.as_bytes()[head.len().min(full.len())..]);
                        wrap_in(lcd, 14, y + 22, 212, y + 18 + h - 4, all.as_bytes(), bg);
                        if focus {
                            let under = if !word.is_empty() { text4_width(word.as_bytes(), &UI_12) } else if pending_tap { 7 } else { 0 };
                            caret(lcd, all.as_bytes(), at, 14, y + 22, 212, under, if matched { BLUE } else { AMBER });
                        }
                    }
                });
            }
            if full_box {
                app.forget(R_LIST_ROW, LIST_VIS);
            }
            if quick_list {
                quick_rows(app, cx.store, lcd, y + 20, self.quick as usize, bg, ver);
            }
        }
        let callsign = cx.store.data().callsign();
        let marks = pred && self.field == 1 && keys.is_empty() && self.punct > 0;
        let strip = (pred, self.field, keys, self.pick, self.pcase, self.case, callsign, quick_list, head, self.list & SVC, (marks, self.punct, full.is_empty()));
        if app.dirty(R_NOTE, Mix::new().any(&strip).get()) {
            let y = 236;
            ferrite_ui::canvas::banded(lcd, 0, y, 240, 60, BG, |lcd| {
                if pred && self.field == 1 && !keys.is_empty() {
                    candidate_strip(lcd, y + 2, keys.as_bytes(), head.as_bytes(), self);
                } else if marks {
                    mark_strip(lcd, y + 2, self.punct);
                } else {
                    let mut b = Buf::<40>::new();
                    if let (Some(k), 1) = (super::contacts::service(d.to.as_bytes()), self.field) {
                        // APRS-39: the service's format
                        let _ = uwrite!(b, "Format: {}", super::contacts::SERVICES[k].2);
                        lcd.text4(8, y + 13, b.as_bytes(), &UI_12, AMBER, BG);
                        b.clear();
                    } else if pred && self.field == 1 {
                        b.push(b"Typing: T9"); // TEXT-10
                    } else {
                        let _ = uwrite!(b, "Typing: {}", ["abc", "ABC", "123"][self.case as usize % 3]);
                    }
                    if !b.is_empty() {
                        lcd.text4(8, y + 13, b.as_bytes(), &UI_12, SUB, BG);
                    }
                }
                match callsign {
                    Some(c) if !self.quick_edit_on() => {
                        let mut f = Buf::<40>::new();
                        let _ = uwrite!(f, "From {}", Txt(c.as_bytes()));
                        lcd.text4(8, y + 37, f.as_bytes(), &UI_12, SUB, BG);
                    }
                    None if !self.quick_edit_on() => {
                        lcd.text4(8, y + 37, "No callsign: set one to send".as_bytes(), &UI_12, AMBER, BG);
                    }
                    _ => {}
                }
                // TEXT-11.1: how to write your own, and the keys the hint bar has no room for
                let help: &[u8] = if quick_list {
                    b"* or a letter: write your own"
                } else if pred && self.field == 1 && full.is_empty() && !self.quick_edit_on() {
                    b"Knob: the quick messages again"
                } else if pred && self.field == 1 && !keys.is_empty() {
                    b"1 mark   hold a digit: digit"
                } else if pred && self.field == 1 {
                    b"* space   hold a digit: digit"
                } else {
                    b"1 . , ? ! ' / - @ : +   * space"
                };
                lcd.text4(8, y + 55, help, &UI_12, DIM, BG);
            });
        }
    }

    fn hints(self, app: &App) -> Hints {
        let pred = self.pred(app) && self.field == 1;
        if self.field == 0 && self.list & LISTED != 0 {
            Hints::new(&[("Knob", "Pick"), ("OK", "Use"), (ARROWS_LR, "Group")]) // APRS-38
        } else if self.field == 0 {
            Hints::new(&[("OK", "Next"), ("#", "Case"), ("BACK", "Delete")])
        } else if pred && !app.text.is_empty() {
            Hints::new(&[("Knob", "Word"), ("*", "Space"), ("#", "abc")]) // TEXT-11
        } else if pred && self.list & LISTED != 0 {
            Hints::new(&[("Knob", "Pick"), ("OK", "Insert"), ("*", "Write")]) // TEXT-6.1
        } else {
            // TEXT-10: # names the mode it goes to: abc, ABC, 123, T9 (a service's message: back to abc)
            let next = match (pred, self.case) {
                (true, _) => "abc",
                (false, 0) => "ABC",
                (false, 1) => "123",
                (false, _) if self.list & SVC != 0 => "abc",
                (false, _) => "T9",
            };
            let ok = if self.quick_edit_on() { ("OK", "Keep") } else { ("OK", "Send") };
            if pred { Hints::new(&[ok, ("1", "Mark"), ("#", next)]) } else { Hints::new(&[ok, ("#", next), ("BACK", "Delete")]) }
        }
    }
}

/// DISP-28: compose's lists repaint by row, as the channel list does: the header, the box, and each of the
/// [`LIST_VIS`] rows in its own region.
const R_LIST_BOX: usize = R_ROW + 1;
const R_LIST_ROW: usize = R_ROW + 2;
const R_LIST_GATE: usize = R_ROW + 2 + LIST_VIS;
const LIST_VIS: usize = 6;

/// The address book on an empty recipient (APRS-38), in the message box's place: which group and where in it,
/// then six rows (group, callsign, and a name, age or what a service does), the highlighted one in blue.
#[allow(clippy::too_many_arguments)]
fn book_rows(app: &mut App, lcd: &mut impl Panel, d: &dyn crate::data::Data, b: &super::contacts::Book, sel: u16, y: i32, h: i32, ver: u32) {
    let n = b.len();
    let sel = sel.min(n - 1);
    let (g, from, len) = b.group(sel);
    if app.dirty(R_ROW, Mix::new().h(&3u8).h(&sel).h(&from).h(&len).h(&g).get()) {
        ferrite_ui::canvas::banded(lcd, 0, y, 240, 18, BG, |lcd| {
            lcd.text4(8, y + 12, "Address book".as_bytes(), &UI_12, SUB, BG);
            let mut c = Buf::<20>::new();
            let _ = uwrite!(c, "{}/{} {}", sel - from + 1, len, super::contacts::GROUPS[g]);
            let cw = text4_width(c.as_bytes(), &UI_12);
            lcd.text4(232 - cw, y + 12, c.as_bytes(), &UI_12, SUB, BG);
        });
    }
    if app.dirty(R_LIST_BOX, Mix::new().h(&3u8).get()) {
        ferrite_ui::canvas::banded(lcd, 0, y + 18, 240, h + 6, BG, |lcd| {
            lcd.fill_r(8, y + 18, 224, h, 2, focus_fill());
            lcd.frame_r(8, y + 18, 224, h, 2, BLUE);
        });
        app.forget(R_LIST_ROW, LIST_VIS);
    }
    let vis = LIST_VIS as u16;
    let top = sel.saturating_sub(vis - 1).min(n.saturating_sub(vis));
    for k in 0..LIST_VIS {
        let i = top + k as u16;
        let on = i == sel;
        if !app.dirty(R_LIST_ROW + k, Mix::new().h(&i).h(&on).h(&ver).h(&(i < n)).get()) {
            continue;
        }
        let ry = y + 22 + k as i32 * 19;
        let (fg, dim, rb) = if on { (BLUE_FG, BLUE_FG, BLUE) } else { (TEXT, SUB, focus_fill()) };
        ferrite_ui::canvas::banded(lcd, 10, ry, 220, 19, focus_fill(), |lcd| {
            let Some((g, call, sub)) = (i < n).then(|| b.row(d, i)).flatten() else { return };
            if on {
                lcd.fill_r(10, ry, 220, 19, 1, BLUE);
            }
            lcd.text4(14, ry + 14, ["Saved", "Recent", "Heard", "Svc"][g].as_bytes(), &UI_12, dim, rb);
            lcd.text4(64, ry + 14, call.as_bytes(), &UI_12B, fg, rb);
            let cx = text4_width(call.as_bytes(), &UI_12B);
            let sw = text4_width(sub.as_bytes(), &UI_12);
            if 64 + cx + 8 + sw <= 226 {
                lcd.text4(226 - sw, ry + 14, sub.as_bytes(), &UI_12, dim, rb);
            }
        });
    }
}

/// The quick messages on an empty message (TEXT-6): the highlighted one in blue, scrolled to keep it in view; each
/// row repaints only when it changes (DISP-28).
#[allow(clippy::too_many_arguments)]
fn quick_rows(app: &mut App, store: &impl ChannelStore, lcd: &mut impl Panel, y: i32, sel: usize, bg: u16, ver: u32) {
    let top = sel.saturating_sub(LIST_VIS - 1).min(crate::words::QUICK - LIST_VIS);
    for k in 0..LIST_VIS {
        let i = top + k;
        let on = i == sel;
        if !app.dirty(R_LIST_ROW + k, Mix::new().h(&i).h(&on).h(&ver).h(&bg).get()) {
            continue;
        }
        let ry = y + 2 + k as i32 * 19;
        let (fg, rb) = if on { (BLUE_FG, BLUE) } else { (TEXT, bg) };
        ferrite_ui::canvas::banded(lcd, 10, ry, 220, 19, bg, |lcd| {
            if on {
                lcd.fill_r(10, ry, 220, 19, 1, BLUE);
            }
            let mut n = Buf::<4>::new();
            let _ = uwrite!(n, "{}", i + 1);
            lcd.text4(16, ry + 14, n.as_bytes(), &UI_12, if on { BLUE_FG } else { SUB }, rb);
            lcd.text4(32, ry + 14, w::fit(quick_text(store, i).as_bytes(), &UI_12, 192), &UI_12, fg, rb);
        });
    }
}

/// The candidate strip (TEXT-11): three at a time, the picked one selected, and where it is in the list; or
/// "no word" (TEXT-3).
fn candidate_strip(lcd: &mut impl Panel, y: i32, keys: &[u8], head: &[u8], s: Compose) {
    let n = candidates(keys, head);
    if n == 0 {
        lcd.text4(8, y + 11, "No word: # to spell it".as_bytes(), &UI_12, AMBER, BG);
        return;
    }
    let from = s.pick as usize / 3 * 3;
    let mut words = [Buf::<16>::new(); 3];
    let mut k = 0;
    for (i, w) in words.iter_mut().enumerate() {
        if let Some((c, caps, _)) = candidate(keys, head, from + i) {
            *w = cased(c.as_bytes(), caps, s.pcase);
            k += 1;
        }
    }
    let items: [&[u8]; 3] = [words[0].as_bytes(), words[1].as_bytes(), words[2].as_bytes()];
    w::chips_sel(lcd, 8, y, 184, &items[..k], Some(s.pick as usize - from), BG);
    let mut c = Buf::<12>::new();
    let _ = uwrite!(c, "{}/{}", s.pick as usize + 1, n);
    let cw = text4_width(c.as_bytes(), &UI_12);
    lcd.text4(232 - cw, y + 13, c.as_bytes(), &UI_12, SUB, BG);
}

#[cfg(test)]
pub(crate) fn marks() -> &'static [u8] {
    PUNCT
}

#[cfg(test)]
pub(crate) fn taps() -> [&'static [u8]; 10] {
    TAPS
}

/// While 1 cycles the marks (TEXT-2.1): every mark, the one in the text highlighted.
fn mark_strip(lcd: &mut impl Panel, y: i32, punct: u8) {
    let mut items: [&[u8]; PUNCT.len()] = [b""; PUNCT.len()];
    for (i, it) in items.iter_mut().enumerate() {
        *it = &PUNCT[i..i + 1];
    }
    w::chips_sel(lcd, 8, y, 224, &items, Some(punct as usize - 1), BG);
}

/// The focused field's and list's fill (DISP-18.2): FOCUS, but none in night red, where FOCUS is solid red and
/// the selected row's red bar was lost in it (a Discord report, ferrite#119): red text on black, the selection the
/// one red bar.
fn focus_fill() -> u16 {
    if ferrite_ui::theme::get() == ferrite_ui::theme::NIGHT_RED { BG } else { FOCUS }
}

fn pop<const N: usize>(b: &mut Buf<N>) {
    let t = *b;
    b.set(&t.as_bytes()[..t.len().saturating_sub(1)]);
}

/// A one-line field: label, a box with the value, a hint when empty.
fn field_box(lcd: &mut impl Panel, y: i32, h: i32, label: &[u8], v: &[u8], focus: bool, pending: bool, empty: &[u8]) {
    ferrite_ui::canvas::banded(lcd, 0, y, 240, h + 26, BG, |lcd| {
        lcd.text4(8, y + 12, label, &UI_12, SUB, BG);
        let bg = if focus { focus_fill() } else { SURFACE };
        lcd.fill_r(8, y + 18, 224, h, 2, bg);
        lcd.frame_r(8, y + 18, 224, h, 2, if focus { BLUE } else { LINE });
        let base = y + 18 + h / 2 + 6;
        if v.is_empty() {
            lcd.text4(16, base, empty, &UI_12, DIM, bg);
            if focus {
                lcd.fill(16, base - 13, 1, 16, BLUE);
            }
        } else {
            let wv = lcd.text4(16, base - 1, v, &UI_15B, TEXT, bg);
            if focus {
                if pending {
                    let lw = text4_width(&v[v.len() - 1..], &UI_15B);
                    lcd.fill(16 + wv - lw, base + 2, lw, 2, BLUE);
                } else {
                    lcd.fill(16 + wv + 1, base - 15, 1, 18, BLUE);
                }
            }
        }
    });
}

/// Wrapped text on a box's colour; returns the lines used.
fn wrap_in(lcd: &mut impl Panel, x: i32, y: i32, w_: i32, y_end: i32, text: &[u8], bg: u16) -> i32 {
    let mut rest = text;
    let mut line = 0;
    while !rest.is_empty() && y + (line + 1) * 17 <= y_end {
        let cut = cut_line(rest, w_);
        lcd.text4(x, y + 13 + line * 17, &rest[..cut], &UI_12, TEXT, bg);
        rest = &rest[cut..];
        line += 1;
    }
    line
}

/// Where a wrapped line ends: after the last space that fits, or where it no longer fits.
fn cut_line(s: &[u8], w_: i32) -> usize {
    if text4_width(s, &UI_12) <= w_ {
        return s.len();
    }
    let fit = w::fit(s, &UI_12, w_).len().max(1);
    match s[..fit].iter().rposition(|&c| c == b' ') {
        Some(p) if p > 0 => p + 1,
        _ => fit,
    }
}

/// The caret at `at` in the wrapped text (TEXT-12; the end when nothing is after the cursor): an underline under
/// the `under` pixels before it (a letter still being tapped, or the predicted word in progress), else a bar.
fn caret(lcd: &mut impl Panel, text: &[u8], at: usize, x: i32, y: i32, w_: i32, under: i32, c: u16) {
    // the line the cursor is on, and its text up to the cursor
    let (mut rest, mut start, mut line) = (text, 0, 0);
    let mut last: &[u8] = b"";
    while !rest.is_empty() {
        let cut = cut_line(rest, w_);
        last = &text[start..at.clamp(start, start + cut)];
        if at < start + cut || rest.len() == cut {
            break;
        }
        (rest, start, line) = (&rest[cut..], start + cut, line + 1);
    }
    let lw = text4_width(last, &UI_12);
    let base = y + 13 + line * 17;
    if under > 0 && !last.is_empty() {
        lcd.fill(x + lw - under.min(lw), base + 2, under.min(lw), 2, c);
    } else {
        lcd.fill(x + lw + 1, base - 12, 1, 15, BLUE);
    }
}

// ---- quick messages (TEXT-7) ----

/// The nine quick messages, from the Messages screen's options: OK edits one, # puts the factory text back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QuickMsgs {
    pub cur: Cursor,
}

impl View for QuickMsgs {
    fn name(self) -> &'static str {
        "Quick messages"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Quick");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let i = self.cur.sel as usize;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) => {
                app.text.clear();
                app.open_screen(Screen::Compose(Compose::quick_edit(i as u8)));
            }
            (Key::Hash, Short) => {
                store.data().request(Op::Quick(i as u8, None));
                app.show_toast("Factory text".as_bytes(), crate::words::quick(i), false);
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let cur = self.cur.step(d, crate::words::QUICK as u16, ROWS2, &ALL);
        app.screen = Screen::QuickMsgs(QuickMsgs { cur });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.page(lcd, 0u8);
        if !app.gate((self, cx.store.data().version())) {
            return;
        }
        app.list_head(lcd, "Shown on an empty message".as_bytes(), b"");
        let rows = self.cur.rows(crate::words::QUICK as u16, ROWS2, &ALL, |r, i| {
            let mut n = Buf::<4>::new();
            let _ = uwrite!(n, "{}", i + 1);
            let own = cx.store.data().quick(i as usize).is_some();
            let q = quick_text(cx.store, i as usize);
            let r2 = Row2 {
                tag: n.as_bytes(),
                title: q.as_bytes(),
                sub: if own { b"yours" } else { b"factory" },
                selected: i == self.cur.sel,
                ..Row2::default()
            };
            app.row2(lcd, r, &r2);
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, crate::words::QUICK as u16, ROWS2, ROW2_H, rows, &ALL);
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "Edit"), ("#", "Factory"), ("BACK", "Back")])
    }
}
