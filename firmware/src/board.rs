//! RT-950 Pro board: every pin the firmware uses, the safe state and start-up. Source: the RT-950 Pro
//! Firmware Reference and analysis/pinmap.md (stock V0.27). (L) = role is likely, not yet confirmed.
//! Carried over from the prototype (fw-rs), which mirrors fw/board.
use crate::hal::gpio::{pin, Mode, Pin, GPIOA, GPIOB, GPIOC, GPIOD, GPIOE};
use crate::hal::{clock, gpio, sys, time, tone};

// RF: all active-high path selects, plus the PA drive (DAC2 output, held at 0 V as a GPIO). Only the Radio
// task raises a receive path; no transmit path is raised in a build without the tx feature.
// The pin tables are `static`s, not `const`s: the safe state and the Radio task read them, and only named statics
// stay in the fast flash (unnamed constants go to the slow part: firmware/memory.x).
pub static RF_PINS: [Pin; 7] = [
    pin(GPIOB, 0),  // VHF RX
    pin(GPIOB, 1),  // VHF TX
    pin(GPIOE, 12), // UHF RX
    pin(GPIOE, 7),  // UHF TX
    pin(GPIOE, 13), // HF RX
    pin(GPIOE, 14), // HF TX
    pin(GPIOA, 5),  // PA drive
];
/// The band relay (Omron G6K): energised for chip 1's HF path below 64 MHz. Only the Radio task's path machine
/// (ferrite_core::path) moves it.
pub const RELAY: Pin = pin(GPIOC, 4);
pub static BK_CS: [Pin; 2] = [pin(GPIOE, 8), pin(GPIOE, 15)]; // BK4829 #0 (VHF), #1 (UHF/HF)
pub const BK_SCK: Pin = pin(GPIOE, 10);
pub const BK_SDA: Pin = pin(GPIOE, 11);

// Audio
pub static AMP_EN: [Pin; 2] = [pin(GPIOB, 8), pin(GPIOE, 4)]; // (L) both
/// PA15: high in stock's receive, low while its crossband repeater transmits: believed to route the receiving
/// chip's audio into the transmitting chip's mic input (`analysis/crossband_repeat.md` §5; to confirm on the
/// bench). Driven only through the path machine's `Act::RepeatAudio` (XB-6, XB-8).
pub const REPEAT_AUDIO: Pin = pin(GPIOA, 15); // (L)
/// PC12, DAC1's mic-line switch (owned by the audio HAL); here for TERM-46's bench `pin`, which drives it as a plain
/// GPIO and never touches the DAC.
#[cfg(feature = "bench")]
pub const MIC_INJECT: Pin = crate::hal::audio::MIC_INJECT;
/// PC9: an audio switch stock's arbiter only ever clears (role unknown; TERM-46 explores it).
#[cfg(feature = "bench")]
pub const PC9_AUDIO: Pin = pin(GPIOC, 9);
/// PC15: stock's debounced, active-low accessory / mute detect (`analysis/pin_audit.md`; docs/hardware.md 6.8): a
/// speaker mic in the side connector grounds it. The input task debounces it (`ferrite_core::acc`, SND-12).
pub const ACC_DET: Pin = pin(GPIOC, 15); // (L: connector pin 3)
/// PE1: stock's audio route switch, high while the speaker mic is connected (PC15 low): the speaker and mic to the
/// side connector, the internal ones off (SND-10). Only the Radio task drives it; low in the safe state.
pub const ACC_ROUTE: Pin = pin(GPIOE, 1); // (L: role from stock's code, to confirm on the bench)

// Power
pub const POWER_HOLD: Pin = pin(GPIOA, 11); // (L) set at start-up by stock, cleared to switch off
pub const POWER_SW: Pin = pin(GPIOE, 0); // power switch sense: low = on, high = switched off (stock: release PA11, reset)

// SPI flash on SPI2 (PB13 SCK, PB14 MISO, PB15 MOSI). Writes only inside ferrite_core::layout::WRITABLE.
pub const FLASH_CS: Pin = pin(GPIOB, 12);

// Broadcast receiver SI4732: bit-banged I2C, 7-bit address 0x11
pub const SI_SCL: Pin = pin(GPIOB, 6);
pub const SI_SDA: Pin = pin(GPIOB, 7);
pub const SI4732_ADDR: u8 = 0x11;
pub const SI_RST: Pin = pin(GPIOA, 6); // SI4732 reset, active low: stock holds it low from boot (pin_audit.md)
/// The SI4732's antenna input path, PA7 and PC5 (manual BCR-20): stock's states are FM broadcast 0/0, AM/SW/SSB 1/1,
/// back to the BK4829 radio 1/0; boot leaves both low. What each state selects is still a bench question.
pub const SI_IN: [Pin; 2] = [pin(GPIOA, 7), pin(GPIOC, 5)];

// GPS on USART3 (PB10 TX, PB11 RX); Bluetooth module on USART1 (PA9 TX, PA10 RX)
pub const GPS_PWR_N: Pin = pin(GPIOA, 8); // low = GPS powered
pub const BT_PWR: Pin = pin(GPIOB, 9); // (L) high = module powered. A GPIO output always: it's TMR4 CH4's pin, the audio
// capture's ADC trigger (`hal::audio`), whose output must never reach it.

// Display: ST7789V, 8080 8-bit, data on PD8-PD15
pub const LCD_WR: Pin = pin(GPIOD, 0);
pub const LCD_CS: Pin = pin(GPIOD, 1);
pub const LCD_RST: Pin = pin(GPIOD, 2);
pub const LCD_DC: Pin = pin(GPIOD, 3);
pub const LCD_DATA_PORT: u32 = GPIOD;

// Indicators
pub const LED_RED: Pin = pin(GPIOC, 13);
pub const LED_GREEN: Pin = pin(GPIOC, 14);
pub const BACKLIGHT: [Pin; 2] = [SCREEN_LIGHT, KEYPAD_LIGHT]; // (L) both
/// The screen's backlight and the keypad light (docs/hardware.md: PB3 is the keypad light). Both start as low
/// outputs; the UI task then hands them to their PWM (`hal::backlight`: PC6 on TMR8 CH1, PB3 on TMR2 CH2 through
/// TMR2's partial remap, which needs JTAG off first; DISP-22, DISP-23).
pub const SCREEN_LIGHT: Pin = pin(GPIOC, 6);
pub const KEYPAD_LIGHT: Pin = pin(GPIOB, 3);
pub const LED_ACTIVE_HIGH: bool = true; // (L) flip if the LEDs turn out active-low

// Keys: matrix drive PC0-PC3, sense PD4-PD7 (active-low, pull-ups)
pub const KEY_DRIVE: [Pin; 4] = [pin(GPIOC, 0), pin(GPIOC, 1), pin(GPIOC, 2), pin(GPIOC, 3)];
pub const KEY_SENSE: [Pin; 4] = [pin(GPIOD, 7), pin(GPIOD, 6), pin(GPIOD, 5), pin(GPIOD, 4)]; // column 0 = PD7
pub const KEY_DIRECT0: Pin = pin(GPIOE, 5); // key code 0
pub const KEY_DIRECT1: Pin = pin(GPIOA, 12); // key code 1
// Stock keys the transmitter from PE3 (PTT A), PE2 (PTT B) and PE6 (PTT on the current VFO, likely the side
// connector's pin 9), and from PE5 when that key is set to PTT C. Ferrite has no PTT C: PE5 (PF1) is SIDE1 and
// PA12 (PF2) SIDE2 (decision 77). Any TX code must unkey only when all PTTs are released.
pub const KEY_PTT: Pin = pin(GPIOE, 3); // PTT A
pub const KEY_SIDE1: Pin = pin(GPIOE, 2); // PTT B
pub const KEY_SIDE2: Pin = pin(GPIOE, 6); // PTT, current VFO (connector PTT, L)
pub const ENC_A: Pin = pin(GPIOB, 4);
pub const ENC_B: Pin = pin(GPIOB, 5);

static SAFE_LOW: [Pin; 9] = [
    RELAY,
    SI_IN[0],
    SI_IN[1],
    pin(GPIOE, 9),
    pin(GPIOC, 12),
    pin(GPIOC, 9),
    ACC_ROUTE,
    pin(GPIOA, 3),
    pin(GPIOB, 2),
];

/// First code after reset and in every fault path: every RF path off, PA drive at 0 V, amps off, chips
/// deselected, power held. Touches no RAM.
pub fn safe_state() {
    gpio::clocks_on();
    // PA drive: hold the DAC in reset so its output cannot override the GPIO below.
    tone::dac_reset();
    for p in RF_PINS {
        p.output(false, Mode::OutPp);
    }
    for p in AMP_EN {
        p.output(false, Mode::OutPp);
    }
    for p in BK_CS {
        p.output(true, Mode::OutPp);
    }
    BK_SCK.output(false, Mode::OutPp);
    FLASH_CS.output(true, Mode::OutPp);
    // Pins stock leaves low that sit next to RF or TX audio: band relay PC4, broadcast-receiver input paths
    // PA7/PC5, PE9 (BK4829/RF cluster), APRS TX audio PC12, audio switches PC9 and PE1 (ACC_ROUTE), and the unused PA3/PB2.
    // Normally the bootloader already left them low; set them so a fault or a start without the bootloader
    // never leaves them floating. PA15 (high in stock RX) is set high in `init` once JTAG is off.
    for p in SAFE_LOW {
        p.output(false, Mode::OutPp);
    }
    POWER_HOLD.output(true, Mode::OutPp);
    // SND-14: PC10 (UART4 TX, the side connector's pin 11) high from reset, as stock's init (0x08016660) drives it:
    // a speaker mic's bias comes from it. Never low; the link task puts it on the UART.
    pin(GPIOC, 10).output(true, Mode::OutPp);
}

/// Called from `__pre_init`, before RAM is initialised (interrupts already masked): the safe state, then
/// the bootloader's SysTick and interrupts off.
pub fn before_ram() {
    safe_state();
    sys::quiesce();
}

/// Everything else the image needs, before any task runs.
pub fn init() {
    clock::init();
    time::init();
    gpio::jtag_off();
    // PA15 is free once JTAG is off: stock's app sets it high at start-up (its receive state; the bootloader leaves
    // it low), so the mic, not the receiver, feeds the transmitter until a crossband pass selects repeat audio.
    REPEAT_AUDIO.output(true, Mode::OutPp);
    LED_RED.output(!LED_ACTIVE_HIGH, Mode::OutPp);
    LED_GREEN.output(!LED_ACTIVE_HIGH, Mode::OutPp);
    for p in BACKLIGHT {
        p.output(false, Mode::OutPp);
    }
    POWER_SW.config(Mode::InPu);
    ACC_DET.config(Mode::InPu); // as stock: input, pull-up
    SI_RST.output(false, Mode::OutPp); // SI4732 held in reset until its driver needs it
    // GPS and Bluetooth modules start powered down; the test pages switch them on.
    GPS_PWR_N.output(true, Mode::OutPp);
    BT_PWR.output(false, Mode::OutPp);
    pin(GPIOB, 10).config(Mode::InFloat);
    pin(GPIOA, 9).config(Mode::InFloat);
    // PWR-23: no input left floating: PA4 (DAC1 audio) analog until its first use, as its users set it; PC7 and
    // PC8 (never read, by stock either) pulled up as stock does.
    pin(GPIOA, 4).config(Mode::Analog);
    pin(GPIOC, 7).config(Mode::InPu);
    pin(GPIOC, 8).config(Mode::InPu);
    // BK4829 data line idle as an input; the SI4732 bus idle high.
    BK_SDA.config(Mode::InPu);
    SI_SCL.config(Mode::InPu);
    SI_SDA.config(Mode::InPu);
}

/// Any fault, stray interrupt or panic: interrupts off first so nothing else runs, then the safe state,
/// red LED on, and a restart (SAFE-8.1; one too many in a row is stopped at the start that follows, SAFE-8.3), or
/// a stop when the stop itself faulted.
pub fn fault() -> ! {
    fault_at(None)
}

/// [`fault`], with the faulting address when known (a HardFault's stacked PC): printed on the cable after the
/// safe state and the LED, and kept for the next start to report (SAFE-8.2).
pub fn fault_at(pc: Option<u32>) -> ! {
    cortex_m::interrupt::disable();
    safe_state();
    LED_RED.output(LED_ACTIVE_HIGH, Mode::OutPp);
    // SAFE-8.3: a restart, the third in a row too: the start that follows stops the radio after keeping the record
    // in the SPI flash, which the fault path never touches (`crate::restart`). Stopped here only when the stop's own
    // keeping faulted.
    let again = crate::restart::fault(pc);
    crate::bringup::fault(pc, again);
    if again {
        crate::hal::uart::flush(crate::report::UART_CPS); // the line out before the reset
        sys::reboot();
    }
    stop()
}

/// SAFE-8.1: one restart too many in a row, found at start-up: the safe state, the red LED, the record kept in the
/// SPI flash for the next start (SAFE-8.3), stopped.
pub fn stopped() -> ! {
    cortex_m::interrupt::disable();
    safe_state();
    LED_RED.output(LED_ACTIVE_HIGH, Mode::OutPp);
    crate::restart::keep();
    crate::bringup::fault(None, false);
    stop()
}

/// Stopped (SAFE-8.1) until the power switch goes off: the watchdog, if running, kept fed so it doesn't restart it.
/// The switch off (PE0 high for [`STOP_OFF_READS`] reads in a row) releases the power hold, as a normal power-off
/// does; before this, only a battery pull got out (ferrite#73). Still powered half a second on (the switch back on
/// before the supply dropped): a fresh start, the restart count cleared as a power cycle clears it. Interrupts are
/// off and the clock may be anything here, so the waits are counted in cycles.
fn stop() -> ! {
    POWER_SW.config(Mode::InPu);
    let mut off = 0u32;
    loop {
        crate::hal::wdt::feed();
        cortex_m::asm::delay(STOP_READ_CYCLES);
        off = if POWER_SW.read() { off + 1 } else { 0 };
        if off >= STOP_OFF_READS {
            POWER_HOLD.output(false, Mode::OutPp);
            for _ in 0..STOP_RELEASE_READS {
                crate::hal::wdt::feed();
                cortex_m::asm::delay(STOP_READ_CYCLES);
            }
            crate::restart::settled();
            sys::reboot();
        }
    }
}

/// [`stop`]'s pace: 10,000 cycles a read (83 us at 120 MHz, 1.25 ms at the 8 MHz reset clock).
const STOP_READ_CYCLES: u32 = 10_000;
/// The switch read off this many times in a row (50 ms at 120 MHz, 0.75 s at 8 MHz): a bounce isn't a switch-off.
const STOP_OFF_READS: u32 = 600;
/// After the power hold is released, this many reads (0.5 s at 120 MHz) before deciding the switch came back on.
const STOP_RELEASE_READS: u32 = 6_000;
