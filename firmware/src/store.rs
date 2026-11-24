//! The SPI-flash store (ferrite-store) on the radio's W25Q: the channels, zones and range presets the UI reads,
//! and the settings and slots it saves (roadmap 1d).
//!
//! Who does what:
//! - The UI task makes the store, mounts it before the boot screen (the callsign and owner card are in the
//!   settings) and reads records on demand, straight from the chip (short reads, about 0.1 ms each).
//! - Every write goes to the store's queue; the Storage task carries it out. Programs and erases are started
//!   and then waited for on a timer, so the UI runs meanwhile and never waits for an erase.
//! - A UI read while the chip is busy suspends the program or erase (W25Q Erase/Program Suspend, 0x75), reads,
//!   and resumes it (0x7A), on a Winbond or Zbit chip only (SAFE-1.3: makers put the suspend flag in different
//!   places, and stock never suspends); on any other chip the read waits for the program or erase to end, as stock does. The
//!   store keeps reads away from the bytes being changed ([`ferrite_store::io`]).
//!
//! The driver here writes only inside ferrite_core::layout::WRITABLE, so calibration, stock's data and the
//! bootloader's update area are refused below everything else. The chip is set up and identified by uradio's
//! `w25q` driver, then driven directly for the suspend, which that driver doesn't have.

use crate::bus::{BusyDelay, Flash};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::once_lock::OnceLock;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use ferrite_app::radio_data::RadioData;
#[cfg(not(feature = "bringup"))]
use ferrite_core::layout::WRITABLE;
use ferrite_store::{FlashStore, Nor, NorError};
use uradio_bus::{Delay, Spi};
use w25q::W25q;

const READ: u8 = 0x03;
const PAGE_PROGRAM: u8 = 0x02;
const SECTOR_ERASE: u8 = 0x20;
const WRITE_ENABLE: u8 = 0x06;
const READ_STATUS1: u8 = 0x05;
const READ_STATUS2: u8 = 0x35;
const SUSPEND: u8 = 0x75;
const RESUME: u8 = 0x7A;
/// tSUS: the longest a suspend takes (Winbond and Zbit alike).
const T_SUS_US: u32 = 20;

/// SAFE-1.3: how a maker's chip shows a suspend, and how long after a resume the next suspend may come.
#[derive(Clone, Copy)]
pub(crate) struct Sus {
    /// Status 2's suspend bits.
    mask: u8,
    /// tRS: the least time from a resume to the next suspend.
    t_rs_us: u32,
}

/// SAFE-1.3: the chips whose suspend we know, by JEDEC maker byte. Winbond keeps one SUS bit, status 2's bit 7, for
/// both; Zbit has two, SUS1 (bit 7) for an erase and SUS2 (bit 2) for a program, and wants 200 us from a resume to
/// the next suspend (ZB25VQ32D sections 6.2.8 and 8.5; Winbond's is 20 us). Zbit is every one of Alex's radios.
pub(crate) fn sus_of(maker: u8) -> Option<Sus> {
    match maker {
        0xEF => Some(Sus { mask: 0x80, t_rs_us: 20 }),
        0x5E => Some(Sus { mask: 0x84, t_rs_us: 200 }),
        _ => None,
    }
}
/// The longest a read waits for a program or erase it can't suspend (several times a slow sector erase).
const WAIT_READ_US: u32 = 2_000_000;

/// Where writes may go: ferrite_core::layout::WRITABLE, or nowhere in the bring-up build.
#[cfg(not(feature = "bringup"))]
pub(crate) const WRITE_OK: &[core::ops::Range<u32>] = &WRITABLE;
#[cfg(feature = "bringup")]
pub(crate) const WRITE_OK: &[core::ops::Range<u32>] = &[];

/// The W25Q (None when it didn't answer at power on: nothing is stored this power-on).
pub struct Chip {
    spi: Option<Flash>,
    size: u32,
    /// A program or erase was started and hasn't been seen to finish.
    active: bool,
    /// SAFE-1.3: a Winbond or Zbit chip, whose program or erase a read may suspend.
    sus: Option<Sus>,
}

impl Chip {
    /// The chip's size in bytes (0 when it didn't answer).
    pub fn size(&self) -> u32 {
        self.size
    }

    fn command(spi: &mut Flash, op: u8) {
        spi.select(true);
        spi.transfer(op);
        spi.select(false);
    }

    fn command_addr(spi: &mut Flash, op: u8, addr: u32) {
        spi.write(&[op, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]);
    }

    fn status(spi: &mut Flash, op: u8) -> u8 {
        spi.select(true);
        spi.transfer(op);
        let s = spi.transfer(0);
        spi.select(false);
        s
    }

    /// SAFE-1.3: until the chip's program or erase ends (busy-polled, as stock does), or [`WAIT_READ_US`].
    fn wait_idle(spi: &mut Flash) -> ferrite_store::Result {
        let mut t = 0;
        while Self::status(spi, READ_STATUS1) & 1 != 0 {
            if t >= WAIT_READ_US {
                return Err(NorError);
            }
            BusyDelay.delay_us(20);
            t += 20;
        }
        Ok(())
    }

    fn start(&mut self, op: u8, addr: u32, data: &[u8]) -> ferrite_store::Result {
        // The bring-up build never programs or erases, whatever asks (the store isn't even mounted there).
        if cfg!(feature = "bringup") {
            return Err(NorError);
        }
        let spi = self.spi.as_mut().ok_or(NorError)?;
        Self::command(spi, WRITE_ENABLE);
        spi.select(true);
        Self::command_addr(spi, op, addr);
        spi.write(data);
        spi.select(false);
        self.active = true;
        Ok(())
    }
}

impl Nor for Chip {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> ferrite_store::Result {
        if addr.checked_add(buf.len() as u32).is_none_or(|e| e > self.size) {
            return Err(NorError);
        }
        let active = self.active;
        let spi = self.spi.as_mut().ok_or(NorError)?;
        let busy = active && Self::status(spi, READ_STATUS1) & 1 != 0;
        // SAFE-1.3: another maker's chip: wait for the program or erase to end (stock's way), then read.
        if busy && self.sus.is_none() {
            Self::wait_idle(spi)?;
            self.active = false;
        }
        // A program or erase running on a Winbond or Zbit chip: suspend it for the read (its own bytes are never read
        // meanwhile). Once a suspend is sent a resume always follows, whatever the status bits said: a chip that
        // finished by itself ignores it, and one whose suspend bit we misread is never left suspended.
        let sus = self.sus.filter(|_| busy);
        if let Some(k) = sus {
            Self::command(spi, SUSPEND);
            let mut t = 0;
            while Self::status(spi, READ_STATUS2) & k.mask == 0 && Self::status(spi, READ_STATUS1) & 1 != 0 && t < 4 * T_SUS_US {
                BusyDelay.delay_us(2);
                t += 2;
            }
            // The suspend didn't take: never read a busy chip (it answers 0xFF or garbage); wait for it instead.
            if Self::status(spi, READ_STATUS1) & 1 != 0 {
                Self::wait_idle(spi)?;
                self.active = false;
            }
        }
        spi.select(true);
        Self::command_addr(spi, READ, addr);
        spi.read(buf);
        spi.select(false);
        if let Some(k) = sus {
            Self::command(spi, RESUME);
            BusyDelay.delay_us(k.t_rs_us);
        }
        Ok(())
    }

    fn start_program(&mut self, addr: u32, data: &[u8]) -> ferrite_store::Result {
        // SAFE-1, SAFE-2: the guard (`ferrite_radio::flash`, host-tested).
        if !ferrite_radio::flash::program_ok(WRITE_OK, addr, data.len() as u32) {
            return Err(NorError);
        }
        self.start(PAGE_PROGRAM, addr, data)
    }

    fn start_erase(&mut self, addr: u32) -> ferrite_store::Result {
        if !ferrite_radio::flash::erase_ok(WRITE_OK, addr) {
            return Err(NorError);
        }
        self.start(SECTOR_ERASE, addr, &[])
    }

    fn busy(&mut self) -> ferrite_store::Result<bool> {
        let spi = self.spi.as_mut().ok_or(NorError)?;
        let busy = Self::status(spi, READ_STATUS1) & 1 != 0;
        self.active &= busy;
        Ok(busy)
    }

    async fn pause() {
        Timer::after_millis(1).await;
    }
}

/// The records the store doesn't hold itself come from `extra` (`ferrite_app::radio_data`): the APRS task's
/// heard list and messages, the callsign, and the GPS.
pub type Store = FlashStore<Chip, RadioData>;

/// The store for the UI and Storage tasks.
pub struct Shared(pub &'static Store);
// SAFETY: the store is touched only by the UI and Storage tasks, which both run on the thread-mode executor:
// one at a time, switching only at an await, and the store holds no borrow across an await.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

/// Set once the UI task has made the store; the Storage task waits for it.
pub static STORE: OnceLock<Shared> = OnceLock::new();
/// Writes are queued: the Storage task has work.
pub static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Starts the flash: the chip woken, left idle and identified; its driver and JEDEC ID and size, or None when it
/// didn't answer (the self-test's SPI flash check).
fn chip() -> (Chip, Option<(u32, u32)>) {
    // SAFE-1.5: a restart (a fault, a hang, the updater) doesn't reset the chip: a program or erase started before it
    // may still be running, and a busy chip ignores the ID read. Woken, then waited for (busy-polled, as stock waits)
    // before it is identified or read.
    let mut flash = Flash::new();
    Chip::command(&mut flash, 0xAB);
    BusyDelay.delay_us(50);
    let _ = Chip::wait_idle(&mut flash);
    let (spi, id) = match W25q::new(flash, &mut BusyDelay, WRITE_OK) {
        Ok(mut f) => {
            let id = (f.jedec_id(), f.size());
            let mut spi = f.release();
            // SAFE-1.5: nor does it end a suspend: a restart between a suspend and its resume leaves the program or
            // erase held, and every later write to it refused. Resumed here, then waited for.
            if let Some(k) = sus_of((id.0 >> 16) as u8) {
                if Chip::status(&mut spi, READ_STATUS2) & k.mask != 0 {
                    Chip::command(&mut spi, RESUME);
                    BusyDelay.delay_us(k.t_rs_us);
                    let _ = Chip::wait_idle(&mut spi);
                }
            }
            (Some(spi), Some(id))
        }
        Err(_) => (None, None),
    };
    let size = id.map_or(0, |i| i.1);
    let sus = id.and_then(|i| sus_of((i.0 >> 16) as u8)); // SAFE-1.3
    (Chip { spi, size, active: false, sus }, id)
}

/// SAFE-8.3: the chip for the restart record's busy-waited reads and writes at start-up and at a safety stop
/// (`crate::restart::keep`), before the store is made; None when it didn't answer. Its writes are held to
/// [`WRITE_OK`] like the store's.
pub fn raw_chip() -> Option<ferrite_store::io::Flash<Chip>> {
    let (c, id) = chip();
    id.map(|_| ferrite_store::io::Flash::new(c))
}

/// Starts the flash and makes the store (not mounted yet: the caller awaits `mount_async`); the chip's JEDEC ID
/// and size, or None when it didn't answer (the self-test's SPI flash check).
#[inline(never)] // SBAR-23: the store is built on this frame's stack before it moves to its static
pub fn make() -> (Option<(u32, u32)>, &'static Store) {
    let (chip, id) = chip();
    let store: &'static Store = cortex_m::singleton!(: Store = FlashStore::new(chip, RadioData::new())).unwrap();
    store.set_deferred(true);
    let _ = STORE.init(Shared(store));
    (id, store)
}

/// The SPI parts' reader (decision 335, `ferrite_core::part`): straight from the chip through the store, a program
/// or erase suspended meanwhile; false before the store is made or while the chip is in use.
pub fn part_read(addr: u32, buf: &mut [u8]) -> bool {
    STORE.try_get().is_some_and(|s| s.0.part_read(addr, buf))
}

/// Tells the Storage task about queued writes.
pub fn wake(store: &Store) {
    if store.has_jobs() {
        WAKE.signal(());
    }
}

/// The radio-wide settings the UI borrows for good (`App::new`): the defaults, then what the store holds
/// ([`ferrite_store::FlashStore::load_settings`]). Once only.
pub fn settings() -> &'static mut ferrite_app::settings::Settings {
    cortex_m::singleton!(: ferrite_app::settings::Settings = ferrite_app::settings::Settings::DEFAULT).unwrap()
}

/// The last alert, for the alert screen (WX-5): the App's too, kept outside it. Once only.
pub fn last_alert() -> &'static mut ferrite_app::screens::alert::AlertInfo {
    cortex_m::singleton!(: ferrite_app::screens::alert::AlertInfo = ferrite_app::screens::alert::AlertInfo::NONE).unwrap()
}
