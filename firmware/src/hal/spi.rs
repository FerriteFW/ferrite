//! SPI2 master, mode 3 as stock (its CTRL1 0x34F: the clock rests high, data read on the rising edge), PB13 SCK /
//! PB14 MISO / PB15 MOSI. Chip selects belong to the drivers.
use super::gpio::{pin, Mode, GPIOB};
use super::regs::*;
use super::{modify, rd, wr};

/// APB1 / 4 = 15 MHz, stock's divider (SPI2 CTRL1 0x34F at 0x08016C00); at another speed (`super::clock`, PWR-24.9)
/// the fastest divider that stays at or under 15 MHz (`ferrite_core::clocktree::Tree::spi_mdiv`).
fn mdiv(t: ferrite_core::clocktree::Tree) -> u32 {
    t.spi_mdiv() << 3
}
const MDIV_MASK: u32 = 7 << 3;

pub fn spi2_init() {
    modify(CRM_APB1EN, 0, CRM_APB1_SPI2);
    wr(SPI2 + SPI_CTRL1, 0);
    pin(GPIOB, 13).output(false, Mode::AfPp);
    pin(GPIOB, 14).config(Mode::InPu);
    pin(GPIOB, 15).output(false, Mode::AfPp);
    // /4 at 120 MHz, 8-bit, mode 3 (SAFE-1.1: stock's mode; it was mode 0 to Nov 19, 2026)
    wr(SPI2 + SPI_CTRL1, SPI_CTRL1_MSTEN | SPI_CTRL1_SWCSEN | SPI_CTRL1_SWCSIL | SPI_CTRL1_CLKPOL | SPI_CTRL1_CLKPHA | mdiv(super::clock::tree()));
    modify(SPI2 + SPI_CTRL1, 0, SPI_CTRL1_SPIEN);
}

pub fn spi2_xfer(out: u8) -> u8 {
    while rd(SPI2 + SPI_STS) & SPI_STS_TDBE == 0 {}
    wr(SPI2 + SPI_DT, out as u32);
    while rd(SPI2 + SPI_STS) & SPI_STS_RDBF == 0 {}
    rd(SPI2 + SPI_DT) as u8
}

/// PWR-24.9: SPI2's divider for the new tree, if it is on (between transactions: the switch runs in thread mode,
/// and every transfer finishes before its task gives way). Off while the divider changes.
pub(super) fn retime(t: ferrite_core::clocktree::Tree) {
    if rd(CRM_APB1EN) & CRM_APB1_SPI2 != 0 && rd(SPI2 + SPI_CTRL1) & SPI_CTRL1_SPIEN != 0 {
        modify(SPI2 + SPI_CTRL1, SPI_CTRL1_SPIEN, 0);
        modify(SPI2 + SPI_CTRL1, MDIV_MASK, mdiv(t));
        modify(SPI2 + SPI_CTRL1, 0, SPI_CTRL1_SPIEN);
    }
}
