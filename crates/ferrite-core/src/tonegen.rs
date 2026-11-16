//! What the BK4829's tone generator plays, worked out on the host: end tones (TONE-10), CW ID (VOX-12), DTMF
//! symbols (DTMF-7, DTMF-11), local beeps (SND-11), and the frequency counter's and the mic meter's readings.
//! The Radio task times the tones; the bk4829 driver writes stock's registers.

use crate::freq::Freq10;

/// One step of a tone sequence: Hz (0 = silence) and how long, ms.
pub type Step = (u16, u16);

/// The end tone choices (TONE-10), index as 0 8 4's list: 0 off. Named statics, each sequence too: the receive tick
/// reads them for the open beep (SQL-20.3), and unnamed constants land in slow flash.
pub static END_TONES: [&[Step]; 6] = [&[], &BEEP, &TWO_UP, &TWO_DOWN, &CHIRP, &QUINDAR];
static BEEP: [Step; 1] = [(1000, 150)];
static TWO_UP: [Step; 2] = [(800, 100), (1200, 100)];
static TWO_DOWN: [Step; 2] = [(1200, 100), (800, 100)];
static CHIRP: [Step; 3] = [(1000, 60), (1250, 60), (1500, 60)];
static QUINDAR: [Step; 1] = [(2475, 250)];

/// CW ID (VOX-12): 700 Hz at 20 wpm.
pub const CW_HZ: u16 = 700;
pub const CW_WPM: u16 = 20;

/// One dit at `wpm` (PARIS: 1200 / wpm ms).
pub const fn dit_ms(wpm: u16) -> u16 {
    1200 / if wpm == 0 { 1 } else { wpm }
}

/// Morse for A-Z, 0-9 and / (the callsign's characters), as dits (1) and dahs (3) packed from the low bits: each
/// element 2 bits (1 dit, 3 dah, 0 end).
fn morse(c: u8) -> u16 {
    const LETTERS: [&[u8]; 26] = [
        b".-", b"-...", b"-.-.", b"-..", b".", b"..-.", b"--.", b"....", b"..", b".---", b"-.-", b".-..", b"--",
        b"-.", b"---", b".--.", b"--.-", b".-.", b"...", b"-", b"..-", b"...-", b".--", b"-..-", b"-.--", b"--..",
    ];
    const DIGITS: [&[u8]; 10] =
        [b"-----", b".----", b"..---", b"...--", b"....-", b".....", b"-....", b"--...", b"---..", b"----."];
    let pat: &[u8] = match c.to_ascii_uppercase() {
        b'A'..=b'Z' => LETTERS[(c.to_ascii_uppercase() - b'A') as usize],
        b'0'..=b'9' => DIGITS[(c - b'0') as usize],
        b'/' => b"-..-.",
        _ => return 0,
    };
    pat.iter().rev().fold(0, |acc, &e| acc << 2 | if e == b'-' { 3 } else { 1 })
}

/// The CW keying of `text`: (key down?, ms) steps, gaps included, at `wpm`. Characters CW can't send are
/// skipped; a space is a word gap.
pub struct Cw<'a> {
    text: &'a [u8],
    i: usize,
    /// The current character's remaining elements, and whether the next step is the gap after an element.
    left: u16,
    gap: bool,
    dit: u16,
}

impl<'a> Cw<'a> {
    pub fn new(text: &'a [u8], wpm: u16) -> Self {
        Cw { text, i: 0, left: 0, gap: false, dit: dit_ms(wpm) }
    }
}

impl Iterator for Cw<'_> {
    type Item = (bool, u16);
    fn next(&mut self) -> Option<(bool, u16)> {
        loop {
            if self.gap {
                self.gap = false;
                // After an element: 1 dit inside a character, 3 between characters (letter gap).
                return Some((false, if self.left == 0 { 3 * self.dit } else { self.dit }));
            }
            if self.left != 0 {
                let e = self.left & 3;
                self.left >>= 2;
                // No gap after the last element of the last character.
                self.gap = self.left != 0 || self.i < self.text.len();
                return Some((true, e * self.dit));
            }
            let c = *self.text.get(self.i)?;
            self.i += 1;
            if c == b' ' {
                return Some((false, 4 * self.dit)); // with the letter gap before it: 7 dits
            }
            self.left = morse(c);
        }
    }
}

/// The total length of a CW ID, ms (for the timeout's count: TONE-11 counts the end tone and ID).
pub fn cw_ms(text: &[u8], wpm: u16) -> u32 {
    Cw::new(text, wpm).map(|(_, ms)| ms as u32).sum()
}

/// The DTMF symbol for the UI's key byte (RadioCmd::Dtmf): digits, A-D (DTMF-11: MENU, ▲, ▼, BACK), * and #.
pub fn dtmf_symbol(key: u8) -> Option<char> {
    match key {
        b'0'..=b'9' | b'A'..=b'D' | b'*' | b'#' => Some(key as char),
        b'a'..=b'd' => Some(key.to_ascii_uppercase() as char),
        _ => None,
    }
}

/// The silent lead-in in tone transmit before an end tone or CW ID (TONE-10, decision 141): the BK4829's generator
/// swells on the first tone after tone transmit is entered (time constant about 80 ms, 90 % at about 190 ms), and
/// tones after that are steady, so the sequence starts this long after the switch.
pub const TONE_LEAD_MS: u16 = 200;

/// Tail elimination (TONE-5): how long the carrier stays up with it, as stock (0x08024DCC keeps the TX 400 ms).
pub const TAIL_MS: u16 = 400;

/// Which tail elimination a transmission ends with (TONE-5): stock's 55 Hz tone with no sub-tone (0x0801D078), the
/// CTCSS phase shift on a CTCSS transmission when 0 8 8 3 picks one, the 134.4 Hz turn-off code on DCS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TailKind {
    Tone55,
    /// REG_52's bits 14:13: 0 the DCS turn-off code, 1-3 the CTCSS phase shift of 120°, 180°, 240°.
    Shift(u8),
}

/// The tail for a transmission with this tone word (CHAN-3: bit 15 DCS) and 0 8 8 3's choice (0 55 Hz, 1-3 the
/// phase shifts): the phase is for CTCSS only.
pub fn tail_kind(tx_tone: u16, phase: u8) -> TailKind {
    match tx_tone {
        0 => TailKind::Tone55,
        t if t & 0x8000 != 0 => TailKind::Shift(0),
        _ if phase == 0 || phase > 3 => TailKind::Tone55,
        _ => TailKind::Shift(phase),
    }
}

/// DTMF-3: a sequence's length, ms: the first digit delay, then each digit on for `ms` and off for the same.
pub fn dtmf_seq_ms(digits: usize, ms: u16, delay_ms: u16) -> u32 {
    delay_ms as u32 + digits as u32 * 2 * ms as u32
}

/// APRS-6.1: the message sounds (0 7 5 5), steps as (Hz, ms); 0 Hz is a rest, 0 ms ends the sound. Played from
/// DAC1 like the key beeps (SND-11), the amp on throughout, two steps louder than the beep volume. Index 0 is Off
/// (no notes). One flat table, no slices: the Radio task reads it, so it sits in fast flash as one named static
/// (slowcheck).
pub static CHIMES: [[(u16, u16); CHIME_STEPS]; 6] = [
    [(0, 0); CHIME_STEPS],
    pad([(1568, 260)]),                                                                   // Ding: one bright note (G6)
    pad([(1047, 110), (1568, 230)]),                                                      // Mail: a rising fifth (C6, G6)
    [(0, 0); CHIME_STEPS],                                                                // Packet: [`PACKET`], not steps
    pad([(2400, 45), (1700, 40), (3100, 35), (0, 25), (1900, 45), (2800, 35), (2200, 40), (3300, 70)]), // Droid
    pad([(1760, 60), (0, 40), (1760, 60), (0, 40), (2093, 110)]),                        // Pager: two chirps, a higher one
];
/// Packet's place in [`CHIMES`]: a short fake APRS burst, 1200 and 2200 Hz a bit or a few at a time.
pub const CHIME_PACKET: u8 = 3;
/// Packet's bits as (Hz, bits), played [`PACKET_REPEAT`] times; a bit is 1 ms (the timer's 0.5 ms ticks; AFSK's is
/// 0.83 ms, which sounds the same).
pub static PACKET: [(u16, u8); 8] = [(1200, 1), (2200, 2), (1200, 1), (2200, 3), (1200, 1), (2200, 2), (1200, 1), (2200, 1)];
pub const PACKET_REPEAT: u8 = 18;
/// The most steps one sound has.
pub const CHIME_STEPS: usize = 10;

/// A sound's steps, the rest of its row ended (0 ms).
const fn pad<const N: usize>(n: [(u16, u16); N]) -> [(u16, u16); CHIME_STEPS] {
    let mut r = [(0, 0); CHIME_STEPS];
    let mut i = 0;
    while i < N {
        r[i] = n[i];
        i += 1;
    }
    r
}

/// APRS-6.1: 0 7 5 6's message volumes as DAC levels (+6 dB a step; 11 is the DAC's full swing), and names. Max
/// by default: the volume knob after the amp's input still sets how loud it is (SND-2).
pub const CHIME_LEVELS: [u8; 4] = [8, 9, 10, 11];
pub const CHIME_VOLUMES: [&str; 4] = ["Low", "Mid", "High", "Max"];

pub const CHIME_NAMES: [&str; 6] = ["Off", "Ding", "Mail", "Packet", "Droid", "Pager"];
/// Packet: Alex, Oct 22, 2026 ("default to packet"; decision 244, a default to review before the first release).
pub const CHIME_DEFAULT: u8 = CHIME_PACKET;

/// A local beep's length, clamped: the Radio task waits through it.
pub const BEEP_MAX_MS: u16 = 1_000;

/// BOOT-18: the boot sound under the boot animation ("Charge and flash" with its "Lock" ending,
/// research/boot-animation/preview.html), steps as (Hz, DAC level 1-11, ms) from [`BOOT_SOUND_AT`] ms after the
/// animation starts. A note per winding as it fills, 25 ms apart: 330 Hz up two octaves over the 14 (2^(1/7) a
/// step), louder as they go; the last glides up a fifth (x1.5) in 10 ms steps into the flash at 630 ms, holds at the
/// peak for 180 ms and fades about a DAC level (6 dB) every 100 ms as the glow settles: no fresh attack. One tone at
/// a time from DAC1, as the key beeps (SND-11), at a fixed level: the volume knob after the amp sets how loud. The
/// preview's gains (0.05 to 0.26, then an exponential fade to 0.002) in 6 dB DAC levels from the peak
/// [`BOOT_SOUND_PEAK`]: level 9 (raised from 8 for loudness, Alex, Oct 28; 6 dB a level) keeps the glide's pitch steps
/// about 3 % at 1.8 kHz (TMR6's divider); level 10 would make them about 6 %, a semitone, audible as steps. The
/// Radio task reads it, so it is one named static (fast flash, slowcheck).
pub static BOOT_SOUND: [(u16, u8, u8); 40] = [
    (330, 7, 25), (364, 7, 25), (402, 7, 25), (444, 7, 25), (490, 7, 25), (541, 7, 25), (598, 8, 25), (660, 8, 25),
    (729, 8, 25), (805, 8, 25), (888, 8, 25), (981, 8, 25), (1083, 8, 25), (1196, 8, 25), // the 14 windings
    (1209, 8, 10), (1237, 8, 10), (1265, 8, 10), (1294, 8, 10), (1323, 8, 10), (1353, 8, 10), (1384, 9, 10),
    (1416, 9, 10), (1448, 9, 10), (1481, 9, 10), (1515, 9, 10), (1549, 9, 10), (1584, 9, 10), (1620, 9, 10),
    (1657, 9, 10), (1695, 9, 10), (1734, 9, 10), (1773, 9, 10), // the glide up a fifth
    (1793, 9, 230), // the flash, held
    (1793, 8, 100), (1793, 7, 110), (1793, 6, 100), (1793, 5, 100), (1793, 4, 100), (1793, 3, 110), (1793, 2, 50), // the fade
];
/// When the boot sound's first note is heard, ms after the animation starts (the first winding fills then); the DAC
/// comes on 30 ms before with the amp still off, as a key beep's (SND-11).
pub const BOOT_SOUND_AT: u32 = 100;
/// The boot sound's loudest DAC level (the flash).
pub const BOOT_SOUND_PEAK: u8 = 9;

/// When the boot sound ends, ms after the animation starts.
pub fn boot_sound_end() -> u32 {
    BOOT_SOUND_AT + BOOT_SOUND.iter().map(|s| s.2 as u32).sum::<u32>()
}

/// The boot sound's step at `t` ms after the animation starts, and when it ends; None before the first note and
/// after the last.
pub fn boot_sound_step(t: u32) -> Option<(usize, u32)> {
    let mut end = BOOT_SOUND_AT;
    if t < end {
        return None;
    }
    for (i, s) in BOOT_SOUND.iter().enumerate() {
        end += s.2 as u32;
        if t < end {
            return Some((i, end));
        }
    }
    None
}

/// Beep volume 1-5 (SND-3, 0 7 5 2) as tone 1's gain: level 3 is stock V0.29's 0x17, each step about 3 dB.
/// Bench-confirm the steps by ear.
pub const BEEP_GAINS: [u8; 5] = [0x0B, 0x10, 0x17, 0x20, 0x2D];

/// The tone gain for beep volume `vol` (1-5; out of range clamps).
pub fn beep_gain(vol: u8) -> u8 {
    BEEP_GAINS[vol.clamp(1, 5) as usize - 1]
}

/// The counter (TSRCH-10): stock (0x08021BC4) takes two frequency-scan readings in a row and keeps them only when
/// they agree within 500 Hz, then the mean. 10 Hz units.
pub fn counter_agree(a: Freq10, b: Freq10) -> Option<Freq10> {
    (a.abs_diff(b) < 50).then(|| ((a as u64 + b as u64) / 2) as Freq10)
}

/// The chip that counts `f`'s band (TSRCH-10): chip 0 for 64-300 MHz, chip 1 from 300 MHz to 1300 MHz outside
/// 630-756 MHz; None outside both (18-64 MHz would need the relay: never counted, SAFE-20).
pub fn counter_chip(f: Freq10) -> Option<u8> {
    use crate::freq::mhz;
    match f {
        f if (mhz(64)..mhz(300)).contains(&f) => Some(0),
        f if (mhz(300)..=mhz(630)).contains(&f) || (mhz(756)..=mhz(1300)).contains(&f) => Some(1),
        _ => None,
    }
}

/// The mic meter (TX-12) from PA0, the mic-level tap stock's VOX reads (decision 76): stock uses the reading's top
/// 8 bits; this many counts is shown as full deviation (to confirm on the bench, with the VOX thresholds' scale:
/// level 1 opens at 101).
pub const MIC_FULL: u16 = 160;

/// PA0's 12-bit reading as % of full deviation, 0-100.
pub fn mic_percent(adc: u16) -> u8 {
    ((adc >> 4).min(MIC_FULL) as u32 * 100 / MIC_FULL as u32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freq::mhz;
    extern crate std;
    use std::vec::Vec;

    /// TONE-10.2: the end tones as listed (Beep, Two-tone up and down, Chirp, Quindar), their lengths, all in the
    /// voice band.
    #[test]
    fn end_tones_as_listed() {
        let ms = |i: usize| END_TONES[i].iter().map(|s| s.1 as u32).sum::<u32>();
        assert!(END_TONES[0].is_empty());
        assert_eq!((ms(1), ms(2), ms(3), ms(4), ms(5)), (150, 200, 200, 180, 250));
        assert_eq!(END_TONES[5], &[(2475, 250)], "Quindar");
        assert!(END_TONES.iter().flat_map(|t| t.iter()).all(|s| (300..=3000).contains(&s.0)), "inside the voice band");
    }

    #[test]
    fn cw_at_20_wpm() {
        assert_eq!(dit_ms(CW_WPM), 60);
        // "E" is one dit; "T" one dah; "ET": dit, letter gap, dah.
        assert_eq!(Cw::new(b"E", 20).collect::<Vec<_>>(), [(true, 60)]);
        assert_eq!(Cw::new(b"ET", 20).collect::<Vec<_>>(), [(true, 60), (false, 180), (true, 180)]);
        // "A": dit, element gap, dah.
        assert_eq!(Cw::new(b"A", 20).collect::<Vec<_>>(), [(true, 60), (false, 60), (true, 180)]);
        // A word gap is 7 dits: the letter gap's 3 plus 4.
        assert_eq!(Cw::new(b"E E", 20).collect::<Vec<_>>(), [(true, 60), (false, 180), (false, 240), (true, 60)]);
        // PARIS is 50 dits including its word gap: 43 without.
        assert_eq!(cw_ms(b"PARIS", 20), 43 * 60);
        // A callsign: its characters only, lower case too, unknown ones skipped.
        assert_eq!(cw_ms(b"va7xyz/p", 20), cw_ms(b"VA7XYZ/P", 20));
        assert_eq!(cw_ms(b"E#E", 20), cw_ms(b"EE", 20));
    }

    #[test]
    fn dtmf_symbols() {
        assert_eq!(dtmf_symbol(b'5'), Some('5'));
        assert_eq!(dtmf_symbol(b'D'), Some('D'), "DTMF-11: BACK sends D");
        assert_eq!(dtmf_symbol(b'#'), Some('#'));
        assert_eq!(dtmf_symbol(b'E'), None);
    }

    #[test]
    fn counter() {
        assert_eq!(counter_agree(mhz(146) + 52_000, mhz(146) + 52_040), Some(mhz(146) + 52_020));
        assert_eq!(counter_agree(mhz(146) + 52_000, mhz(146) + 52_050), None, "500 Hz apart: not the same carrier");
        assert_eq!(counter_chip(mhz(146)), Some(0));
        assert_eq!(counter_chip(mhz(446)), Some(1));
        assert_eq!(counter_chip(mhz(700)), None, "the chip's gap");
        assert_eq!(counter_chip(mhz(1296)), Some(1));
        assert_eq!(counter_chip(mhz(28)), None, "HF needs the relay");
    }

    #[test]
    fn mic_meter() {
        assert_eq!(mic_percent(0), 0);
        assert_eq!(mic_percent(80 << 4), 50);
        assert_eq!(mic_percent(0xFFF), 100);
    }

    /// TONE-18: tail elimination as stock: 55 Hz with no sub-tone, the phase shift on CTCSS, the DCS turn-off
    /// code on DCS.
    #[test]
    fn tails_and_dtmf_timing() {
        assert_eq!(tail_kind(0, 2), TailKind::Tone55, "no sub-tone: 55 Hz whatever the phase");
        assert_eq!(tail_kind(885, 0), TailKind::Tone55);
        assert_eq!(tail_kind(885, 2), TailKind::Shift(2), "CTCSS: the phase shift");
        assert_eq!(tail_kind(0x8000 | 0o23, 2), TailKind::Shift(0), "DCS: the turn-off code, phase or not");
        assert_eq!(dtmf_seq_ms(4, 100, 300), 1100);
    }

    /// SND-3: volume 3 is stock's beep gain; the scale rises and clamps.
    #[test]
    fn beep_volume_gains() {
        assert_eq!(beep_gain(3), 0x17);
        assert!(BEEP_GAINS.windows(2).all(|w| w[0] < w[1]));
        assert_eq!((beep_gain(0), beep_gain(9)), (0x0B, 0x2D));
    }

    /// Lower-case A-D send as upper case; a speed of 0 counts as 1 wpm (no division by zero); a chime row is its
    /// notes, then ended (0 ms).
    #[test]
    fn small_helpers() {
        assert_eq!(dtmf_symbol(b'a'), Some('A'));
        assert_eq!(dtmf_symbol(b'd'), Some('D'));
        assert_eq!(dtmf_symbol(b'e'), None);
        assert_eq!(dit_ms(0), 1200);
        let row = pad([(440, 10), (0, 5)]);
        assert_eq!(&row[..3], &[(440, 10), (0, 5), (0, 0)]);
        assert_eq!(CHIMES[2], pad([(1047, 110), (1568, 230)]));
    }

    /// BOOT-18.2: the boot sound as the preview plays it ("Lock"): a note per winding 25 ms apart from 100 ms, 330 Hz
    /// up 2^(1/7) a step; the last glides up a fifth by the flash at 630 ms, never down; the level rises to the peak
    /// and then only falls (no fresh attack); over by about 1.5 s, inside the boot screen's 1.5 s plus its tail.
    #[test]
    fn boot_sound_charges_glides_and_fades() {
        for (i, s) in BOOT_SOUND[..14].iter().enumerate() {
            let want = 330.0 * 2f64.powf(i as f64 / 7.0);
            assert!((s.0 as f64 - want).abs() < 1.0, "note {i}: {} Hz, want {want:.0}", s.0);
            assert_eq!(s.2, 25, "note {i}");
        }
        assert!(BOOT_SOUND.windows(2).all(|w| w[1].0 >= w[0].0), "the pitch only rises");
        let flash = boot_sound_step(630).unwrap().0;
        assert_eq!(BOOT_SOUND[flash].0, 1793, "a fifth over the last note at the flash");
        assert!((BOOT_SOUND[flash].0 as f64 / BOOT_SOUND[13].0 as f64 - 1.5).abs() < 0.01);
        assert_eq!(boot_sound_step(629).map(|s| BOOT_SOUND[s.0].0), Some(1773), "still gliding before it");
        let peak = BOOT_SOUND.iter().position(|s| s.1 == BOOT_SOUND_PEAK).unwrap();
        assert!(BOOT_SOUND[..peak].windows(2).all(|w| w[1].1 >= w[0].1), "louder into the flash");
        assert!(BOOT_SOUND[peak..].windows(2).all(|w| w[1].1 <= w[0].1), "then only quieter");
        assert!(BOOT_SOUND.iter().all(|s| (1..=11).contains(&s.1) && s.0 > 0 && s.2 > 0));
        assert_eq!(boot_sound_end(), 1530);
        assert_eq!(boot_sound_step(99), None);
        assert_eq!(boot_sound_step(100), Some((0, 125)));
        assert_eq!(boot_sound_step(1529).map(|s| s.0), Some(BOOT_SOUND.len() - 1));
        assert_eq!(boot_sound_step(1530), None);
    }
}
