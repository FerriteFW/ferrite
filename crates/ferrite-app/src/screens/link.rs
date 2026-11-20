//! Bluetooth and the link (manual chapter 29): link status (D17, MENU 0 9 2 2; LINK-17) and a write session's
//! progress (D21; LINK-9). The Link task opens the progress with [`App::open`] (`Dest::WriteProgress`) and reads
//! BACK from [`App::take_link_answer`]. Access is the cable, or Bluetooth with the radio's PIN while one is set
//! (LINK-7, LINK-30): no approval prompt and no approved-devices list (decision 104).

use super::shared::*;
use super::table::{Ctx, Table, Val};
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::App;
use ferrite_core::keys::Key;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkPage {
    Status,
    Progress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Link {
    pub page: LinkPage,
    pub row: u8,
}

/// The user's answer on the progress screen, for the Link task (LINK-9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAnswer {
    Cancelled,
}

const ROWS: [(&str, Val<()>); 4] = [
    ("Bluetooth", |Ctx { app: a, dev: d, .. }, b| {
        if d.bt {
            b.set(b"Connected");
            BLUE
        } else {
            b.set(if a.set.bt { b"On, waiting".as_slice() } else { b"Off" });
            TEXT
        }
    }),
    ("Cable", |Ctx { dev: d, .. }, b| {
        if !d.link.cable {
            b.set(b"Not connected");
            return SUB;
        }
        b.set(b"Connected"); // always trusted (LINK-7)
        BLUE
    }),
    ("Writes", |Ctx { dev: d, .. }, b| {
        match d.link.write_pct {
            Some(p) => {
                let _ = uwrite!(b, "Writing, {} %", p);
            }
            None => b.set(b"Idle"),
        }
        TEXT
    }),
    // LINK-30: Bluetooth asks for the radio's PIN while the PIN lock is on.
    ("BT access", |Ctx { app: a, .. }, b| {
        b.set(if a.pin_required() { b"PIN".as_slice() } else { b"Open" });
        TEXT
    }),
];

pub(crate) const STATUS: Table<()> = Table { rows: &ROWS, go: &[], y: 22, h: 20, style: w::KV_STATUS };

impl Link {
    pub fn new(page: LinkPage) -> Self {
        Link { page, row: 0 }
    }
}

impl View for Link {
    fn name(self) -> &'static str {
        match self.page {
            LinkPage::Status => "Link status",
            LinkPage::Progress => "Writing",
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(match self.page {
            LinkPage::Status => b"Link".as_slice(),
            LinkPage::Progress => b"Writing",
        });
    }

    fn modal(self) -> bool {
        self.page == LinkPage::Progress // LINK-9: keys ignored but BACK
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        match (self.page, e.key, e.press) {
            (LinkPage::Progress, Key::Exit, Short | Long) => {
                app.link_answer = Some(LinkAnswer::Cancelled);
                app.back();
                app.show_toast(b"Write cancelled", b"", false);
            }
            (LinkPage::Progress, ..) => {}
            (_, Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        if down && self.page != LinkPage::Progress {
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let dev = cx.dev;
        match self.page {
            LinkPage::Status => {
                app.table(lcd, &STATUS, cx.live, dev, &(), None);
                app.help_txt(lcd, 160, 48, txt!("Ferrite Console connects by cable or Bluetooth."), b"");
            }
            LinkPage::Progress => {
                let pct = dev.link.write_pct.unwrap_or(100);
                app.message(lcd, b"Writing", b"from Ferrite Console", b"Keys are ignored while Console writes. BACK cancels.", TEXT);
                if app.dirty(R_NOTE, Mix::new().h(&pct).get()) {
                    let mut t = Buf::<8>::new();
                    let _ = uwrite!(t, "{} %", pct);
                    w::progress(lcd, 30, 210, 180, 6, pct, BLUE);
                    w::field(lcd, 10, 220, 222, 240, 236, t.as_bytes(), &w::st(&ferrite_ui::assets::UI_12B, TEXT, BG, Align::Center));
                }
            }
        }
    }

    fn hints(self, _: &App) -> Hints {
        match self.page {
            LinkPage::Progress => Hints::new(&[("BACK", "Cancel")]),
            LinkPage::Status => Hints::new(&[("BACK", "Back")]),
        }
    }
}
