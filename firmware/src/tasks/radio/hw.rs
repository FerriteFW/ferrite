//! The path machine's hardware steps.

use super::*;

/// The pins, waits and chip steps a path plan moves ([`ferrite_radio::run`]).
impl ferrite_radio::Hw for Radio {
    fn pin(&mut self, p: Pin, high: bool) {
        match p {
            Pin::Sel(s) => RF_PINS[s as usize].write(high),
            Pin::PaDrive => RF_PINS[6].write(high),
            Pin::Relay => RELAY.write(high),
            Pin::SiIn(i) => SI_IN[i as usize & 1].write(high),
            Pin::RepeatAudio => REPEAT_AUDIO.write(high),
            Pin::Amp(0) => pb8(high),
            Pin::Amp(_) => AMP_EN[1].write(high),
        }
    }
    fn relay_is(&mut self) -> bool {
        RELAY.read()
    }
    async fn wait_ms(&mut self, ms: u32) {
        Timer::after_millis(ms as u64).await
    }
    fn muted(&mut self, m: bool) {
        self.muted = m;
    }
    fn si_used(&mut self) -> bool {
        self.si_used
    }
    fn set_si_used(&mut self) {
        self.si_used = true;
    }
    fn log(&mut self, tag: &[u8], v: u32) {
        log(tag, v);
    }
    #[cfg(feature = "tx")]
    fn pa_zero(&mut self) {
        crate::hal::pa::zero();
    }
    #[cfg(feature = "tx")]
    fn pa_set(&mut self, v: u8) {
        crate::hal::pa::set(v);
    }
    #[cfg(feature = "tx")]
    fn pa_level(&mut self) -> u8 {
        self.tx.pa
    }
    #[cfg(feature = "tx")]
    fn key_chip(&mut self, n: u8) -> bool {
        Radio::key_chip(self, n)
    }
    #[cfg(feature = "tx")]
    fn chip_fault(&mut self, _n: u8) {
        self.tx.fault = true;
    }
    #[cfg(feature = "tx")]
    fn unkey_chip(&mut self, n: u8) {
        if self.has(n) {
            self.chip(n).tx_off();
            // VOX-15.3: a raised pre-emphasis gain back to stock's (receive and the next key-up start from it)
            if self.tx.pre_raised.take() == Some(n) {
                ferrite_radio::chip::pre_gain_stock(&mut self.chip(n));
            }
        }
    }
    // TONE-8's order at the unkey, still keyed: end tone, CW ID when due, tail elimination.
    #[cfg(feature = "tx")]
    async fn end_tone(&mut self) {
        let e = self.tx.end[self.tx.slot as usize % 4];
        let t = tonegen::END_TONES.get(e as usize).copied().unwrap_or(&[]);
        self.play_keyed(t.iter().copied()).await;
    }
    #[cfg(feature = "tx")]
    async fn cw_id(&mut self) {
        let (call, len) = (self.tx.cw_call, self.tx.cw_len as usize);
        let cw = tonegen::Cw::new(&call[..len.min(10)], tonegen::CW_WPM);
        self.play_keyed(cw.map(|(on, ms)| (if on { tonegen::CW_HZ } else { 0 }, ms))).await;
    }
    #[cfg(feature = "tx")]
    async fn tail_elim(&mut self) {
        Radio::tail_elim(self).await
    }
}
