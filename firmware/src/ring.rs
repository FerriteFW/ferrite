//! Single-producer, single-consumer byte ring (an interrupt fills it, one task drains it). Lock-free: head
//! is written only by the producer, tail only by the consumer. The buffer length must be a power of two;
//! one slot stays empty to tell full from empty. Atomics all the way down, so no `unsafe`.
use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering::*};

pub struct Ring<'a> {
    buf: &'a [AtomicU8],
    mask: u16,
    head: AtomicU16,
    tail: AtomicU16,
    dropped: AtomicU32,
}

impl<'a> Ring<'a> {
    pub const fn new(buf: &'a [AtomicU8]) -> Self {
        assert!(buf.len().is_power_of_two() && buf.len() <= 1 << 15);
        let mask = (buf.len() - 1) as u16;
        Ring { buf, mask, head: AtomicU16::new(0), tail: AtomicU16::new(0), dropped: AtomicU32::new(0) }
    }

    /// Empty, nothing dropped (only while the producer is stopped).
    pub fn reset(&self) {
        self.head.store(0, Relaxed);
        self.tail.store(0, Relaxed);
        self.dropped.store(0, Relaxed);
    }

    /// Producer side. False (and counted as dropped) when full.
    pub fn put(&self, b: u8) -> bool {
        let h = self.head.load(Relaxed);
        let next = (h + 1) & self.mask;
        if next == self.tail.load(Acquire) {
            self.dropped.fetch_add(1, Relaxed);
            return false;
        }
        self.buf[h as usize].store(b, Relaxed);
        self.head.store(next, Release);
        true
    }

    /// Consumer side.
    pub fn get(&self) -> Option<u8> {
        let t = self.tail.load(Relaxed);
        if t == self.head.load(Acquire) {
            return None;
        }
        let b = self.buf[t as usize].load(Relaxed);
        self.tail.store((t + 1) & self.mask, Release);
        Some(b)
    }

    pub fn count(&self) -> u16 {
        self.head.load(Relaxed).wrapping_sub(self.tail.load(Relaxed)) & self.mask
    }

    /// Consumer side: drop everything waiting.
    pub fn clear(&self) {
        self.tail.store(self.head.load(Acquire), Release);
    }

    /// Producer side: bytes that can be put without one being dropped.
    pub fn room(&self) -> u16 {
        self.mask - self.count()
    }

    /// Producer side: a byte not put, counted as dropped.
    pub fn drop_one(&self) {
        self.dropped.fetch_add(1, Relaxed);
    }

    pub fn dropped(&self) -> u32 {
        self.dropped.load(Relaxed)
    }
}
