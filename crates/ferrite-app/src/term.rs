//! The terminal and mini CPS over the cable (manual chapter 37, TERM-1 to TERM-20; Term* mock-ups): a VT100
//! interface any terminal program can use at 80 x 24 or more. A third renderer of the radio's state after the
//! screen and the web app: the main menu, channels (list and editor), zones, the settings with the radio's own
//! numbers and validation (TERM-6, TERM-7), a monitor of the event log and live activity, and About.
//!
//! No screen buffer (TERM-19): every row is drawn on demand from the App and the store, and only a hash of each
//! row is kept, so a row is sent again only when what it shows changed (the clock, a slot opening, an edit made
//! on the radio: TERM-10). The state is a few hundred bytes. It never blocks (TERM-4): bytes in through
//! [`Term::feed`], rows out through [`Term::poll`] while the [`Sink`] has room; a row that doesn't fit waits for
//! the next poll.
//!
//! Slice 1 (Oct 4, 2026): the pages. Slice 2: the command line (`:`, TERM-12) and dump/load (TERM-13), in
//! [`cmd`]; a command that answers in several lines (a dump, a load's report, help) uses the console, which
//! streams lines and then waits for Enter. Not yet: marking or colour (TERM-14, 18).

use crate::chan::ChannelStore;
pub(crate) use crate::chfield::parse_mhz;
use crate::settings::CABLE_PORTS;
use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_core::VERSION;
use ferrite_ui::text::{Buf, Zero};

/// Text from a template (in slow flash) and its arguments into a buffer, through the terminal's one formatter
/// ([`cmd::fmt_into`]): no `uwrite!` expansion per call site.
macro_rules! put {
    ($o:expr, $t:literal) => {{
        use $crate::term::cmd::Fmt as _;
        $o.fill($t, &[])
    }};
    ($o:expr, $t:literal, $($a:expr),+ $(,)?) => {{
        use $crate::term::cmd::Fmt as _;
        $o.fill($t, &[$($crate::term::cmd::Arg::from($a)),+])
    }};
}

pub mod cmd;
pub mod debug;
#[cfg(feature = "vt100")]
mod pages;
#[cfg(feature = "vt100")]
mod natx;
#[cfg(feature = "vt100")]
mod tpl;
#[cfg(feature = "vt100")]
use pages::*;
mod line;

/// The template pack as built with this crate (host builds: the packer writes it as the `.ffw`'s template part).
#[cfg(all(feature = "vt100", not(target_os = "none")))]
pub use tpl::body as template_pack;

#[cfg(test)]
std::thread_local! {
    /// Tests: the template part missing (TERM-47's plain command line in a vt100 build).
    pub(crate) static NO_VT: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// Where the terminal's output goes: the cable's transmit buffer.
pub trait Sink {
    /// Bytes that can be put now without blocking.
    fn room(&self) -> usize;
    fn put(&mut self, b: &[u8]);
}

/// Terminal size limits (TERM-1, TERM-15): 80 x 24 at least; larger terminals use what they have, up to these.
pub const MIN_COLS: u8 = 80;
pub const MAX_COLS: u8 = 200;
pub const MIN_ROWS: u8 = 24;
pub const MAX_ROWS: u8 = 64;
/// A lone ESC is the Esc key once nothing follows it for this long (arrows and PF keys start with ESC).
const ESC_MS: u32 = 60;
/// Every row is checked again this often: the clock, live activity, edits made on the radio (TERM-10).
#[cfg(feature = "vt100")]
const REFRESH_MS: u32 = 1000;
/// The most a row can take on the wire: its text plus the cursor move and attributes.
const ROW_MAX: usize = MAX_COLS as usize + 48;

/// What the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Not started: in *auto* the cable waits for a typed Enter (TERM-2).
    Off,
    Splash,
    Main,
    /// The slots and the active one's quick settings (TERM-48).
    Radio,
    Channels,
    Zones,
    Settings,
    /// APRS heard and messages (TERM-49).
    Aprs,
    /// The GPS position (TERM-50).
    Gps,
    Monitor,
    About,
    Edit,
}

/// A decoded key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    Ch(u8),
    Up,
    Down,
    Left,
    Right,
    PgUp,
    PgDn,
    Home,
    End,
    Enter,
    Esc,
    Bs,
    CtrlC,
    CtrlL,
    Pf(u8),
}

/// Where an escape sequence is: none, after ESC, after ESC [, after ESC O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seq {
    None,
    Started,
    Csi,
    Ss3,
}

/// The console (several lines of answer: a dump, a load, help) instead of the pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Con {
    Off,
    /// Clear the screen first.
    Start(Body),
    Run(Body),
    /// Esc or Ctrl-C: say so, then wait.
    Stop,
    /// Over: Enter or Esc goes back to the page.
    #[cfg_attr(not(feature = "vt100"), allow(dead_code))]
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    List(cmd::List, u16),
    Dump(cmd::DumpPos),
    /// Lines read, applied and refused; `end` 1 when it ended, 2 when it was stopped.
    Load { line: u16, ok: u16, bad: u16, end: u8 },
    /// A debug command waiting on, or printing, the firmware's answers (TERM-21).
    Debug(debug::Job),
}

fn cmd_trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|c| *c != b' ').unwrap_or(s.len());
    let b = s.iter().rposition(|c| *c != b' ').map_or(a, |k| k + 1);
    &s[a..b]
}

pub struct Term {
    page: Page,
    /// The full-screen pages: a vt100 build with its template part there (TERM-47, UPD-21; decided at start);
    /// otherwise the plain command line.
    vt: bool,
    /// Ctrl-C: say goodbye on the next poll and wait for Enter again.
    bye: bool,
    /// The pages' state (the template program's variables, marks, the channel edited).
    #[cfg(feature = "vt100")]
    pg: tpl::Pages,
    /// The note under the page: what a change did or why it was refused.
    note: cmd::Msg,
    warn: bool,
    /// The note is a command's answer: the bottom row shows it until the next key.
    bnote: bool,
    /// The command line (`:`, TERM-12) and `load`'s line (TERM-13) share one buffer; `clast` is the last
    /// command's length while it is still there (Up brings it back).
    cbuf: [u8; cmd::LINE_MAX],
    clen: u8,
    clast: u8,
    /// The command line is open on the bottom row.
    cmd: bool,
    /// A whole line waits in the buffer for the next poll to run it (after the store has caught up).
    pending: bool,
    /// `sat add` (SAT-3): the load's lines are a TLE or OMM CSV, and what the paste holds between them.
    sat: Option<cmd::SatPaste>,
    /// A load's line was longer than the buffer; the line before was ended by CR (its LF is no new line).
    long: bool,
    cr: bool,
    con: Con,
    cols: u8,
    rows: u8,
    /// Rows to check, and the hash of what each row last sent (0 = never sent).
    dirty: u64,
    #[cfg(feature = "vt100")]
    hash: [u32; MAX_ROWS as usize],
    clear: bool,
    #[cfg(feature = "vt100")]
    ask_size: bool,
    esc: Seq,
    esc_at: u32,
    num: [u16; 2],
    nn: u8,
    #[cfg(feature = "vt100")]
    last: u32,
    /// The time of the last poll (ms): debug answers time out against it.
    now: u32,
    /// Bluetooth (TERM-25): auto start only (the cable port setting is the cable's). While the radio's PIN lock is
    /// on, a Bluetooth terminal asks for the PIN first and does nothing else until it's right (LINK-30, LINK-31):
    /// whether it's needed, given, and the digits typed so far.
    bt: bool,
    pin_need: bool,
    pin_ok: bool,
    pin: [u8; 6],
    pin_n: u8,
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

impl Term {
    pub const fn new() -> Self {
        Term {
            page: Page::Off,
            vt: false,
            bye: false,
            #[cfg(feature = "vt100")]
            pg: tpl::Pages::new(),
            note: Buf::new(),
            warn: false,
            bnote: false,
            cbuf: [0; cmd::LINE_MAX],
            clen: 0,
            clast: 0,
            cmd: false,
            pending: false,
            sat: None,
            long: false,
            cr: false,
            con: Con::Off,
            cols: MIN_COLS,
            rows: MIN_ROWS,
            dirty: 0,
            #[cfg(feature = "vt100")]
            hash: [0; MAX_ROWS as usize],
            clear: false,
            #[cfg(feature = "vt100")]
            ask_size: false,
            esc: Seq::None,
            esc_at: 0,
            num: [0; 2],
            nn: 0,
            #[cfg(feature = "vt100")]
            last: 0,
            now: 0,
            bt: false,
            pin_need: false,
            pin_ok: false,
            pin: [0; 6],
            pin_n: 0,
        }
    }

    /// What it shows now ([`Page::Off`] until it has been started).
    #[cfg(not(feature = "vt100"))]
    pub fn page(&self) -> Page {
        self.page
    }

    /// The size in use: columns, rows.
    pub fn size(&self) -> (u8, u8) {
        (self.cols, self.rows)
    }

    /// The terminal's size from outside (the web simulator's pane); a cursor position report does the same.
    pub fn set_size(&mut self, cols: u16, rows: u16) {
        self.cols = cols.clamp(MIN_COLS as u16, MAX_COLS as u16) as u8;
        self.rows = rows.clamp(MIN_ROWS as u16, MAX_ROWS as u16) as u8;
        self.redraw();
    }

    /// Rows are still waiting for room in the sink, a line waits to run, or the console is streaming: poll
    /// again soon.
    pub fn busy(&self) -> bool {
        self.page != Page::Off && (self.dirty != 0 || self.clear || self.bye || self.pending || self.bulk_on() || matches!(self.con, Con::Start(_) | Con::Run(_)))
    }

    /// It takes another byte now. False while a whole line waits to run (a load waits for the store between
    /// lines): leave the bytes where they are, the cable's flow control holds the sender (TERM-4, TERM-13).
    pub fn ready(&self) -> bool {
        !self.pending
    }

    /// The terminal is open (Enter started it and Ctrl-C hasn't ended it).
    pub fn on(&self) -> bool {
        self.page != Page::Off
    }

    /// Which port this terminal runs on: Bluetooth starts on Enter whatever the cable port setting says (TERM-25).
    pub fn set_bluetooth(&mut self, bt: bool) {
        self.bt = bt;
    }

    /// LINK-30: this terminal must be given the radio's PIN before anything else (Bluetooth with the PIN lock on).
    /// Set each poll; the PIN given lasts for this terminal (a new connection opens a new one).
    pub fn set_pin_needed(&mut self, need: bool) {
        self.pin_need = need;
    }

    /// Open and still waiting for the PIN (LINK-30).
    fn pin_gate(&self) -> bool {
        self.page != Page::Off && self.pin_need && !self.pin_ok
    }

    /// LINK-31: one byte typed at the PIN prompt: digits (shown as *), Backspace, Enter checks it against the
    /// radio's PIN with its counter and wait, Ctrl-C leaves. Nothing else is read.
    fn pin_byte(&mut self, app: &mut App, b: u8) {
        match b {
            b'0'..=b'9' if (self.pin_n as usize) < self.pin.len() => {
                self.pin[self.pin_n as usize] = b;
                self.pin_n += 1;
            }
            0x08 | 0x7F => self.pin_n = self.pin_n.saturating_sub(1),
            b'\r' | b'\n' if self.pin_n > 0 => {
                let n = core::mem::take(&mut self.pin_n) as usize;
                match app.remote_pin(&self.pin[..n]) {
                    crate::screens::pin::RemotePin::Ok => {
                        self.pin_ok = true;
                        self.note.clear();
                        self.redraw();
                        return;
                    }
                    crate::screens::pin::RemotePin::Wrong => self.say("Wrong PIN".as_bytes(), true),
                    crate::screens::pin::RemotePin::Wait(w) => {
                        let mut b = Buf::<40>::new();
                        put!(b, "Too many tries: wait {} s", w);
                        self.say(b.as_bytes(), true);
                    }
                }
                self.pin = [0; 6];
            }
            0x03 => self.bye = true,
            _ => {}
        }
        self.all_dirty();
    }

    /// Everything again: clear the screen, forget what each row showed (Ctrl-L, TERM-16).
    fn redraw(&mut self) {
        self.clear = true;
        #[cfg(feature = "vt100")]
        {
            self.hash = [0; MAX_ROWS as usize];
        }
        self.all_dirty();
    }

    /// Several channels changing (TERM-14): the poll has more to do.
    #[cfg(not(feature = "vt100"))]
    fn bulk_on(&self) -> bool {
        false
    }

    fn all_dirty(&mut self) {
        self.dirty = if self.rows >= 64 { u64::MAX } else { (1u64 << self.rows) - 1 };
    }

    fn start(&mut self) {
        *self = Term { cols: self.cols, rows: self.rows, bt: self.bt, pin_need: self.pin_need, ..Term::new() };
        #[cfg(feature = "vt100")]
        {
            #[cfg(test)]
            let no = NO_VT.with(|v| v.get());
            #[cfg(not(test))]
            let no = false;
            self.vt = tpl::present() && !no;
        }
        if self.vt {
            self.page = Page::Splash;
            #[cfg(feature = "vt100")]
            {
                self.ask_size = true;
            }
        } else {
            self.page = Page::Main; // the command line: the banner, then a prompt
        }
        self.redraw();
    }

    fn say(&mut self, s: &[u8], warn: bool) {
        self.note.set(s);
        (self.warn, self.bnote) = (warn, false);
    }

    // ---- Input ----

    /// One byte from the cable, `now` in ms. Call the App's `pop_write` after each byte: a channel edit writes
    /// one channel per key.
    pub fn feed<S: ChannelStore>(&mut self, app: &mut App, b: u8, now: u32, store: &S, dev: &Device, live: &LiveAll) {
        if self.page == Page::Off {
            // TERM-2: auto starts on a typed Enter; terminal on any byte; link and KISS never. COBS link frames
            // (the 0x00 delimiter) select the link protocol in milestone 3.
            let go = match if self.bt { 0 } else { app.set.cable_port } {
                0 => b == b'\r' || b == b'\n',
                2 => true,
                _ => false,
            };
            if go {
                self.start();
            }
            return;
        }
        if self.pin_gate() {
            return self.pin_byte(app, b);
        }
        if let Con::Run(Body::Load { .. }) = self.con {
            return self.load_byte(b);
        }
        let k = match (self.esc, b) {
            (Seq::None, 0x1B) => {
                (self.esc, self.esc_at) = (Seq::Started, now);
                return;
            }
            (Seq::Started, b'[') => {
                (self.esc, self.num, self.nn) = (Seq::Csi, [0; 2], 0);
                return;
            }
            (Seq::Started, b'O') => {
                self.esc = Seq::Ss3;
                return;
            }
            (Seq::Started, _) => {
                // ESC then something else: the Esc key, then that byte.
                self.esc = Seq::None;
                self.key(app, K::Esc, store, dev, live);
                return self.feed(app, b, now, store, dev, live);
            }
            (Seq::Csi, b'0'..=b'9') => {
                let i = (self.nn as usize).min(1);
                self.num[i] = self.num[i].saturating_mul(10).saturating_add((b - b'0') as u16);
                return;
            }
            (Seq::Csi, b';') => {
                self.nn = self.nn.saturating_add(1);
                return;
            }
            (Seq::Csi, _) => {
                self.esc = Seq::None;
                match b {
                    b'A' => K::Up,
                    b'B' => K::Down,
                    b'C' => K::Right,
                    b'D' => K::Left,
                    b'H' => K::Home,
                    b'F' => K::End,
                    b'R' => {
                        // The cursor position report: the terminal's size (TERM-15).
                        let (r, c) = (self.num[0], self.num[1]);
                        if r > 0 && c > 0 {
                            self.set_size(c, r);
                        }
                        return;
                    }
                    b'~' => match self.num[0] {
                        1 | 7 => K::Home,
                        4 | 8 => K::End,
                        5 => K::PgUp,
                        6 => K::PgDn,
                        11..=14 => K::Pf((self.num[0] - 10) as u8),
                        _ => return,
                    },
                    _ => return,
                }
            }
            (Seq::Ss3, _) => {
                self.esc = Seq::None;
                match b {
                    b'P'..=b'S' => K::Pf(b - b'P' + 1),
                    b'A' => K::Up,
                    b'B' => K::Down,
                    b'C' => K::Right,
                    b'D' => K::Left,
                    b'H' => K::Home,
                    b'F' => K::End,
                    b'M' => K::Enter,
                    _ => return,
                }
            }
            (Seq::None, b'\r') => K::Enter,
            (Seq::None, b'\n') => return, // CR LF terminals: the CR was the Enter
            (Seq::None, 0x08 | 0x7F) => K::Bs,
            (Seq::None, 0x03) => K::CtrlC,
            (Seq::None, 0x0C) => K::CtrlL,
            (Seq::None, 0x20..=0x7E) => K::Ch(b),
            _ => return,
        };
        self.key(app, k, store, dev, live);
    }

    /// A byte of a load (TERM-13): lines, not keys. Ctrl-C stops, Ctrl-D or a line `end` finishes.
    fn load_byte(&mut self, b: u8) {
        let Con::Run(Body::Load { line, ok, bad, .. }) = self.con else { return };
        let end = |t: &mut Term, how: u8| t.con = Con::Run(Body::Load { line, ok, bad, end: how });
        let was_cr = core::mem::replace(&mut self.cr, b == b'\r');
        match b {
            0x03 => end(self, 2),
            0x04 => end(self, 1),
            b'\n' if was_cr => {}
            b'\r' | b'\n' => {
                if cmd_trim(&self.cbuf[..self.clen as usize]) == b"end" {
                    self.clen = 0;
                    end(self, 1);
                } else {
                    self.pending = true;
                }
            }
            b'\t' | 0x20..=0x7E => {
                if (self.clen as usize) < cmd::LINE_MAX {
                    self.cbuf[self.clen as usize] = if b == b'\t' { b' ' } else { b };
                    self.clen += 1;
                } else {
                    self.long = true;
                }
            }
            _ => {}
        }
    }

    /// Keys on the console: Enter or Esc leaves it when it is done; Esc or Ctrl-C stops a list or dump.
    fn con_key(&mut self, k: K) {
        match (self.con, k) {
            (Con::Done, K::Enter | K::Esc | K::CtrlC) => {
                self.con = Con::Off;
                self.redraw();
            }
            (Con::Start(_) | Con::Run(_), K::Esc | K::CtrlC) => self.con = Con::Stop,
            (_, K::CtrlL) => {}
            _ => {}
        }
    }

    /// Keys on the command line.
    fn cmd_key(&mut self, k: K) {
        match k {
            K::Enter => {
                self.cmd = false;
                if self.clen > 0 {
                    (self.pending, self.clast) = (true, self.clen);
                }
            }
            K::Esc => self.cmd = false,
            K::Bs => self.clen = self.clen.saturating_sub(1),
            K::Up if self.clen == 0 => self.clen = self.clast,
            K::Ch(c) if (self.clen as usize) < cmd::LINE_MAX => {
                if self.clen < self.clast {
                    self.clast = 0; // typing over the last command: Up has nothing to bring back
                }
                self.cbuf[self.clen as usize] = c;
                self.clen += 1;
            }
            _ => {}
        }
    }

    fn key<S: ChannelStore>(&mut self, app: &mut App, k: K, store: &S, dev: &Device, live: &LiveAll) {
        self.all_dirty();
        if self.con != Con::Off {
            return self.con_key(k);
        }
        self.bnote = false;
        if self.cmd && k != K::CtrlC {
            return self.cmd_key(k);
        }
        match k {
            K::CtrlC => {
                self.bye = true;
                return;
            }
            K::CtrlL => return self.redraw(),
            _ => {}
        }
        if self.vt {
            #[cfg(feature = "vt100")]
            self.page_key(app, k, store, dev, live);
        } else {
            let _ = (app, store, dev, live);
            self.line_key(k);
        }
    }

    // ---- Output ----

    /// A debug command's next request for the firmware (TERM-21), once: memory is read by the caller, the
    /// BK4829 and SI4732 requests go to the Radio task. Answer with [`Term::debug_answer`].
    pub fn debug_req(&mut self) -> Option<ferrite_core::msg::DebugReq> {
        match &mut self.con {
            Con::Run(Body::Debug(j)) => j.request(self.now),
            _ => None,
        }
    }

    /// The firmware's answer to the last [`Term::debug_req`]; ignored once the command has been stopped.
    pub fn debug_answer(&mut self, a: ferrite_core::msg::DebugAns) {
        if let Con::Run(Body::Debug(j)) = &mut self.con {
            j.answer(a);
        }
    }

    /// Sends what changed while the sink has room: the goodbye, a clear, then each row whose text changed.
    #[inline(never)] // its own stack frame, not the caller's for its whole run (SBAR-23)
    pub fn poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, now: u32, out: &mut impl Sink) {
        self.now = now;
        if self.esc == Seq::Started && now.wrapping_sub(self.esc_at) >= ESC_MS {
            self.esc = Seq::None;
            self.key(app, K::Esc, store, dev, live);
        }
        if self.page == Page::Off {
            return;
        }
        if self.bye {
            if out.room() < 96 {
                return;
            }
            out.put(b"\x1b[0m\x1b[2J\x1b[H");
            out.put("Ferrite terminal closed. Press Enter to open it again.\r\n".as_bytes());
            *self = Term { cols: self.cols, rows: self.rows, ..Term::new() };
            return;
        }
        #[cfg(feature = "vt100")]
        if self.vt && now.wrapping_sub(self.last) >= REFRESH_MS {
            self.last = now;
            self.all_dirty();
        }
        #[cfg(feature = "vt100")]
        if self.vt && self.ask_size {
            if out.room() < 32 {
                return;
            }
            // Save the cursor, go far down right, ask where it is, come back (TERM-15).
            out.put(b"\x1b7\x1b[999;999H\x1b[6n\x1b8");
            self.ask_size = false;
        }
        #[cfg(feature = "vt100")]
        if self.vt && !self.page_poll(app, store, dev, live, out) {
            return;
        }
        // A whole line: run it once the store has taken the last one's writes (TERM-13 never drops a line).
        if self.pending && !app.write_pending() && !store.data().busy() && out.room() >= 2 * ROW_MAX {
            self.pending = false;
            self.exec(app, store, dev, out);
        }
        if self.con != Con::Off {
            return self.console(app, store, dev, live, out);
        }
        if self.vt {
            #[cfg(feature = "vt100")]
            self.draw(app, store, dev, live, out);
        } else {
            self.line_prompt(out);
        }
    }

    /// Runs the line in the buffer: a typed command, or a load's next line (its result and the count go out).
    fn exec<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, out: &mut impl Sink) {
        let n = self.clen as usize;
        self.clen = if self.con == Con::Off { self.clen } else { 0 };
        if let Con::Run(Body::Load { mut line, mut ok, mut bad, end }) = self.con {
            line += 1;
            let r = if core::mem::take(&mut self.long) {
                Err(cmd::Msg::from("too long: 255 characters at most".as_bytes()))
            } else if let Some(p) = self.sat.as_mut() {
                cmd::sat_line(store, p, &self.cbuf[..n])
            } else {
                cmd::run(app, store, dev, &self.cbuf[..n], true)
            };
            match r {
                Ok(cmd::Done::Nothing) => {}
                Ok(_) => ok += 1,
                Err(m) => {
                    bad += 1;
                    let mut l = cmd::Line::new();
                    put!(l, "\r\x1b[K  line {}: ", line);
                    l.push(m.as_bytes());
                    out.put(l.as_bytes());
                    out.put(b"\r\n");
                }
            }
            self.con = Con::Run(Body::Load { line, ok, bad, end });
            let mut l = Buf::<64>::new();
            put!(l, "\r\x1b[K  {} lines read: {} applied, {} refused", line, ok, bad);
            out.put(l.as_bytes());
            return;
        }
        let (m, warn) = match cmd::run(app, store, dev, &self.cbuf[..n], false) {
            Ok(cmd::Done::Nothing) => return,
            Ok(cmd::Done::Said(m)) => (m, false),
            Ok(cmd::Done::List(l)) => return self.con = Con::Start(Body::List(l, 0)),
            Ok(cmd::Done::Dump) => return self.con = Con::Start(Body::Dump(cmd::DumpPos::default())),
            Ok(cmd::Done::Debug(j)) => return self.con = Con::Start(Body::Debug(j)),
            Ok(d @ (cmd::Done::Load | cmd::Done::SatPaste)) => {
                // The buffer takes the load's lines from here: the command that started it is gone.
                (self.clen, self.clast, self.cr, self.long) = (0, 0, false, false);
                self.sat = matches!(d, cmd::Done::SatPaste).then(cmd::SatPaste::default);
                return self.con = Con::Start(Body::Load { line: 0, ok: 0, bad: 0, end: 0 });
            }
            Err(m) => (m, true),
        };
        if self.vt {
            #[cfg(feature = "vt100")]
            {
                (self.note, self.warn, self.bnote) = (m, warn, true);
            }
        } else {
            let _ = warn;
            out.put(b"\r\x1b[K  ");
            out.put(m.as_bytes());
            out.put(b"\r\n");
            self.clen = 0;
        }
        self.all_dirty();
    }

    /// The console: a clear screen, then lines while the sink has room; then "Enter returns".
    fn console<S: ChannelStore>(&mut self, app: &App, store: &S, dev: &Device, live: &LiveAll, out: &mut impl Sink) {
        const LINE_ROOM: usize = 300;
        match self.con {
            Con::Off | Con::Done => {}
            Con::Start(b) => {
                if out.room() < LINE_ROOM {
                    return;
                }
                out.put(if self.vt { b"\x1b[0m\x1b[2J\x1b[H" } else { b"\r\x1b[K" });
                if let (Body::Load { .. }, Some(_)) = (b, self.sat) {
                    out.put(crate::text::load(txt!("Paste satellites: a TLE (name, line 1, line 2) or OMM CSV lines. One with a catalogue number\r\nheld gets the new elements; another is added. It ends at \"end\" or Ctrl-D; Ctrl-C stops.\r\n\r\n")).as_bytes());
                } else if let Body::Load { .. } = b {
                    out.put(crate::text::load(txt!("Load: send the dump now. A bad line is skipped and named. It ends at \"end\" or Ctrl-D;\r\nCtrl-C stops. Use XON/XOFF flow control, or a short delay per line.\r\n\r\n")).as_bytes());
                }
                self.con = Con::Run(b);
                self.console(app, store, dev, live, out);
            }
            Con::Run(Body::Load { line, ok, bad, end }) => {
                if end != 0 && !self.pending && out.room() >= LINE_ROOM {
                    let mut l = cmd::Line::new();
                    let how = if end == 2 { "Stopped" } else { "Done" };
                    self.sat = None;
                    put!(l, "\r\x1b[K  {}: {} lines read, {} applied, {} refused.\r\n", how, line, ok, bad);
                    out.put(l.as_bytes());
                    self.done(out);
                }
            }
            Con::Run(mut b) => {
                while out.room() >= LINE_ROOM {
                    let mut l = cmd::Line::new();
                    let more = match &mut b {
                        Body::List(what, i) => {
                            let m = cmd::list_line(app, store, dev, live, *what, *i, &mut l);
                            *i += 1;
                            m
                        }
                        Body::Dump(p) => cmd::dump_line(app, store, dev, p, &mut l),
                        Body::Load { .. } => false,
                        Body::Debug(j) => match j.line(self.now, &mut l) {
                            debug::Step::Line => true,
                            debug::Step::Wait => break,
                            debug::Step::Over => false,
                        },
                    };
                    if !more {
                        return self.done(out);
                    }
                    out.put(l.as_bytes());
                    out.put(b"\r\n");
                }
                self.con = Con::Run(b);
            }
            Con::Stop => {
                if out.room() >= LINE_ROOM {
                    out.put("\r\n  Stopped.\r\n".as_bytes());
                    self.pending = false;
                    self.sat = None;
                    self.done(out);
                }
            }
        }
    }

    fn done(&mut self, out: &mut impl Sink) {
        if self.vt {
            out.put("\r\n  Enter or Esc returns to the menu.".as_bytes());
            (self.con, self.clen) = (Con::Done, 0);
        } else {
            (self.con, self.clen) = (Con::Off, 0);
            self.all_dirty(); // the prompt again
        }
    }

}

// ---- Helpers ----

/// Settings the terminal leaves to the radio: they open their own screens there (the PIN, Extend TX's warning)
/// or the cable may not change them (the region: TERM-7; Dry TX, TX-48: the PC never decides whether a key-up has
/// power).
fn blocked(p: &[u8]) -> bool {
    matches!(p, [9, 3] | [9, 3, 1] | [8, 8, 4] | [8, 8, 5] | [9, 4])
}

/// "0.8.0" from "v0.8.0".
fn version() -> &'static [u8] {
    VERSION.strip_prefix('v').unwrap_or(VERSION).as_bytes()
}

/// "146.52000": MHz with five decimals (10 Hz steps), as the channel list shows it.
fn mhz5(f: Freq10) -> Buf<12> {
    let mut b = Buf::new();
    put!(b, "{}.{}", f / 100_000, Zero(f % 100_000, 5));
    b
}

use crate::screens::Screen;

/// The channel open in one of the radio's editors (TERM-17): the active slot's channel while its channel
/// editor, or an editor opened from it, is on the radio's screen.
pub(crate) fn radio_channel(app: &App) -> Option<u16> {
    let from_editor = app.back.iter().flatten().any(|s| matches!(s, Screen::ChEdit(_)));
    let open = match app.screen {
        Screen::ChEdit(_) => true,
        Screen::Value(_) | Screen::Tones(_) | Screen::Text(_) | Screen::Entry(_) => from_editor,
        _ => false,
    };
    let s = &app.slots[app.active as usize];
    s.ch.filter(|_| open && s.on_channel()).map(|c| c.num)
}

/// The setting highlighted in the radio's menu (TERM-17): the knob changes it there at once (MENU-16).
pub(crate) fn radio_setting(app: &App) -> Option<([u8; 3], usize)> {
    match app.screen {
        Screen::Group(g) if g.sub == 0 => Some(([g.g, g.row + 1, 0], 2)),
        Screen::Group(g) => Some(([g.g, g.sub, g.row + 1], 3)),
        _ => None,
    }
}

/// The cable port setting's names (0 9 2 3), for the radio's menu too.
pub fn port_name(p: u8) -> &'static str {
    CABLE_PORTS[p as usize % CABLE_PORTS.len()]
}

/// `n` in decimal, written into the end of `d`: one formatter for every number the terminal shows.
#[inline(never)]
pub(crate) fn digits(mut n: u32, d: &mut [u8; 10]) -> &[u8] {
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &d[i..];
        }
    }
}

#[cfg(all(test, feature = "vt100"))]
mod tests;
#[cfg(test)]
mod tests_line;
/// The command line and the keystrokes fuzzed (ferrite-fuzzkit).
#[cfg(test)]
mod tests_fuzz;
