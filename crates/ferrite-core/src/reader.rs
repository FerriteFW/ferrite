//! The reader image's protocol (BACKUP-9.1, BACKUP-9.4, BACKUP-9.5; decision 264): its console lines, `fdump`'s
//! frames and the CRC-32 they carry. Byte for byte what the hwtest reader spoke (fw/src/core/fdump.h,
//! fw/apps/hwtest/svc.c), so Ferrite Console, fw/tools/backup.py and flashdump.py read either image unchanged.
//!
//! The console (115200 baud, 8N1): a line per command, echoed as typed, each answer ending with the `> ` prompt.
//! - `sys`: one line with ` uid ` and the MCU's 96-bit unique ID, three words lowest first (24 hex digits).
//! - `flash`: `flash id EF4016 (Winbond, 4096 KB) status 00 uid ...  cal sector sum ...`.
//! - `flashcrc`: the radio's own CRC-32 of the whole chip, `flash crc32 1A2B3C4D over 4096 KB, ...`.
//! - `fdump <addr> <len> [baud]`: the binary dump below, at `baud` (500000 when left out).
//! - `mdump <addr> <len>`: the MCU's own flash, read only, as hwtest's: a line per 32 bytes, `80F0000:` (the
//!   address, 6 hex digits at least), the bytes in hex, `*` and their byte sum, then `dump: end`. The backup reads
//!   stock's marker sector with it, `mdump 0x080F0000 16` (BACKUP-12, decision 338); only the main flash
//!   (0x08000000 up, as big as the chip says) is read.
//! - `reader`: the backup screen ("reader screen on"); `reboot`: back through the bootloader; `off`.
//!
//! `fdump`:
//! 1. The answer is one line, `fdump <addr> <len> <baud> ready, ...` (or `fdump: ...` with the reason it won't);
//!    then the radio switches to `baud`.
//! 2. The host switches too and sends `FDS1` until the radio answers `FDOK` (3 s, else the radio goes back).
//! 3. Blocks of up to [`BLOCK`] bytes, each a frame: [`HDR`] bytes of header (`F` `D` type 0, address u32,
//!    length u16, little-endian), the data, then the CRC-32 (zlib's) of header and data, u32 LE. The host answers
//!    ACK (next block), NAK (send it again) or CAN (stop); silence for [`ACK_MS`] also resends, [`TRIES`] sends at
//!    most per frame. After a resent frame's ACK the radio waits [`DUP_ACK_MS`] and drops what came: a second ACK
//!    for a second copy the host got.
//! 4. An end frame: type `E`, the start address, length 16, then start u32, length u32, the CRC-32 of all the
//!    data, the number of resends u32; acknowledged like a block.
//! 5. The radio goes back to 115200, waits [`RESULT_PAUSE_MS`] for the host to switch too, and prints
//!    `fdump end crc32 XXXXXXXX ...` (or `fdump stopped at ...`).

pub const BLOCK: usize = 4096;
pub const HDR: usize = 10;
pub const ACK: u8 = 0x06;
pub const NAK: u8 = 0x15;
pub const CAN: u8 = 0x18;
pub const TRIES: u32 = 10;
pub const ACK_MS: u32 = 2000;
pub const SYNC_MS: u32 = 3000;
/// From the rate switch back to 115200 to the result line: the host's own switch (Web Serial closes and reopens
/// the port, slow on Windows) must be done first, or the line is lost (FerriteFW/console#87).
pub const RESULT_PAUSE_MS: u32 = 500;
pub const TYPE_BLOCK: u8 = b'B';
pub const TYPE_END: u8 = b'E';
/// `fdumpz`'s frame: one [`ZCHUNK`] of the range, coded (BACKUP-9.6): a mode byte, then the data.
pub const TYPE_ZBLOCK: u8 = b'Z';
/// `fdumpz`'s chunk: 16 KB read, coded and sent as one frame (fewer frames, fewer waits for an ACK).
pub const ZCHUNK: usize = 16384;
/// The coded chunk's room: the mode byte and the chunk raw (the coder falls back to raw when it can't do better).
pub const ZBUF: usize = ZCHUNK + 1;
pub const Z_RAW: u8 = 0;
pub const Z_ERASED: u8 = 1;
/// Raw DEFLATE (RFC 1951): one final block with the fixed Huffman codes. Every host inflates it with what it has
/// (Python's zlib, a browser's `DecompressionStream("deflate-raw")`).
pub const Z_DEFLATE: u8 = 4;
/// [`deflate_fixed`]'s hash table of positions: 2^13 entries, 16 KB.
pub const DEFLATE_HASH_BITS: u32 = 13;

/// A whole `fdumpz` frame: header, mode byte and chunk at most, CRC.
pub const ZFRAME: usize = HDR + ZBUF + 4;

/// `fdumpz`'s working memory (about 48 KB): two whole frames, one on the cable while the next is coded into the
/// other, and the encoder's hash table. The radio keeps it in static memory, so a frame being sent from the
/// interrupt is never on a stack that moves on.
pub struct ZWork {
    pub frames: [[u8; ZFRAME]; 2],
    pub head: [u16; 1 << DEFLATE_HASH_BITS],
}

impl ZWork {
    pub const fn new() -> Self {
        Self { frames: [[0; ZFRAME]; 2], head: [0; 1 << DEFLATE_HASH_BITS] }
    }
    /// The first frame's coded chunk (mode byte first): what [`zcode`] wrote.
    pub fn coded(&self) -> &[u8] {
        &self.frames[0][HDR..]
    }
}

impl Default for ZWork {
    fn default() -> Self {
        Self::new()
    }
}
pub const SYNC: [u8; 4] = *b"FDS1";
pub const SYNC_OK: [u8; 4] = *b"FDOK";
/// The console's rate, and `fdump`'s when none is given.
pub const CONSOLE_BAUD: u32 = 115_200;
pub const FDUMP_BAUD: u32 = 500_000;
/// `fdumpz`'s rate when none is given (BACKUP-9.6, decision 265): one fixed, safe rate. At 460800 sustained
/// transfers lost bytes through a USB extension (black, Oct 23); safer beats a few seconds.
pub const FDUMPZ_BAUD: u32 = 230_400;

/// The help text. Ferrite Console tells the reader image from others by its "fdump <addr> <len>" (READER_MARK).
pub const HELP: &[u8] = b"sys | flash | flashcrc   (SPI flash, read only)\r\n\
fdump <addr> <len> [baud]   (SPI flash, read only, binary: fw/tools/flashdump.py --fast)\r\n\
fdumpz <addr> <len> [baud]  (as fdump, 8 KB chunks coded: erased, run-length or raw)\r\n\
mdump <addr> <len>          (MCU flash, read only)\r\n\
reader | reboot | off\r\n";

/// The MCU's main flash, where `mdump` reads.
pub const MCU_FLASH: u32 = 0x0800_0000;
/// Stock's marker sector (BACKUP-12, decision 338): the backup records its first [`MARKER_LEN`] bytes.
pub const MARKER_ADDR: u32 = 0x080F_0000;
pub const MARKER_LEN: u32 = 16;
/// Bytes per `mdump` line.
pub const MDUMP_LINE: usize = 32;

// ---- CRC-32 ----

const fn table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

/// A named static (fast flash: firmware/memory.x's rule for tables read per byte).
static CRC_TABLE: [u32; 256] = table();

/// zlib's CRC-32, continued from `crc` (start at 0): `crc32(crc32(0, a), b) == crc32(0, a ++ b)`. hwtest's
/// `crc32_update`, so `flashcrc` and the frames agree with Console's own CRC of what it read.
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

// ---- frames ----

/// A frame's header.
pub fn header(kind: u8, addr: u32, len: u16) -> [u8; HDR] {
    let a = addr.to_le_bytes();
    let l = len.to_le_bytes();
    [b'F', b'D', kind, 0, a[0], a[1], a[2], a[3], l[0], l[1]]
}

/// The end frame's payload.
pub fn end_payload(start: u32, len: u32, data_crc: u32, resends: u32) -> [u8; 16] {
    let mut p = [0u8; 16];
    for (i, v) in [start, len, data_crc, resends].into_iter().enumerate() {
        p[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    p
}

/// The CRC that closes a frame: header and data.
pub fn frame_crc(hdr: &[u8], data: &[u8]) -> [u8; 4] {
    crc32(crc32(0, hdr), data).to_le_bytes()
}

/// Watches the cable for the host's `FDS1`, with any noise before it.
#[derive(Default)]
pub struct Sync {
    got: usize,
}

impl Sync {
    /// True once the whole `FDS1` has come.
    pub fn feed(&mut self, b: u8) -> bool {
        self.got = if b == SYNC[self.got] { self.got + 1 } else if b == SYNC[0] { 1 } else { 0 };
        if self.got == SYNC.len() {
            self.got = 0;
            return true;
        }
        false
    }
}

/// The cable and a clock, as `fdump` needs them (the radio's UART4 and cycle counter; a scripted host in the tests).
pub trait Link {
    fn write(&mut self, b: &[u8]);
    /// Starts sending a whole frame and may return before it has gone ([`Link::sent`] waits): the caller leaves
    /// `frame` alone until then. A plain write by default.
    fn send(&mut self, frame: &[u8]) {
        self.write(frame)
    }
    /// Waits until what [`Link::send`] started has gone.
    fn sent(&mut self) {}
    /// The next byte within `ms`, or None.
    fn getc(&mut self, ms: u32) -> Option<u8>;
    /// Drops whatever has come and not been taken.
    fn clear_rx(&mut self);
    /// Milliseconds, monotonic.
    fn ms(&mut self) -> u32;
    fn pause(&mut self, ms: u32);
}

/// A finished dump.
#[derive(Debug, PartialEq)]
pub struct Dumped {
    pub crc: u32,
    pub blocks: u32,
    pub resends: u32,
    pub ms: u32,
}

/// Why a dump stopped, and where.
#[derive(Debug, PartialEq)]
pub struct Stopped {
    pub at: u32,
    pub why: &'static str,
}

/// DEFLATE's bit order: values least significant bit first, Huffman codes most significant bit first.
struct Bits<'a> {
    out: &'a mut [u8],
    o: usize,
    acc: u32,
    n: u32,
    full: bool,
}

impl Bits<'_> {
    fn put(&mut self, v: u32, n: u32) {
        self.acc |= v << self.n;
        self.n += n;
        while self.n >= 8 {
            match self.out.get_mut(self.o) {
                Some(b) => *b = self.acc as u8,
                None => self.full = true,
            }
            self.o += 1;
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    fn code(&mut self, code: u32, n: u32) {
        self.put(code.reverse_bits() >> (32 - n), n)
    }
    /// The fixed literal/length code for `v` (0-287).
    fn lit(&mut self, v: u32) {
        match v {
            0..=143 => self.code(0x30 + v, 8),
            144..=255 => self.code(0x190 + v - 144, 9),
            256..=279 => self.code(v - 256, 7),
            _ => self.code(0xC0 + v - 280, 8),
        }
    }
}

const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];

/// The longest earlier match for `src[i..]` from the hash table (`i` entered into it): (length, distance), or
/// (0, 0) under 3 bytes.
fn find(src: &[u8], head: &mut [u16; 1 << DEFLATE_HASH_BITS], i: usize) -> (usize, usize) {
    if i + 3 > src.len() {
        return (0, 0);
    }
    let h = hash3(src, i);
    let c = head[h];
    head[h] = i as u16;
    if c == u16::MAX {
        return (0, 0);
    }
    let c = c as usize;
    let max = (src.len() - i).min(258);
    let mut l = 0;
    while l < max && src[c + l] == src[i + l] {
        l += 1;
    }
    if l >= 3 { (l, i - c) } else { (0, 0) }
}

fn hash3(src: &[u8], p: usize) -> usize {
    let v = src[p] as u32 | (src[p + 1] as u32) << 8 | (src[p + 2] as u32) << 16;
    (v.wrapping_mul(2_654_435_761) >> (32 - DEFLATE_HASH_BITS)) as usize
}

/// BACKUP-9.6: `src` (at most 64 KB) as raw DEFLATE, one final fixed-Huffman block, into `out`; None when it
/// doesn't fit. Greedy matches from one hash table with one step of lazy matching (zlib's idea: a longer match one
/// byte on wins).
pub fn deflate_fixed(src: &[u8], out: &mut [u8], head: &mut [u16; 1 << DEFLATE_HASH_BITS]) -> Option<usize> {
    head.fill(u16::MAX);
    let mut b = Bits { out, o: 0, acc: 0, n: 0, full: false };
    b.put(1, 1); // BFINAL
    b.put(1, 2); // BTYPE 01: fixed Huffman codes
    let n = src.len();
    let mut i = 0;
    let mut pending = None;
    while i < n {
        let (l, d) = pending.take().unwrap_or_else(|| find(src, head, i));
        if l >= 3 {
            if l < 32 && i + 1 < n {
                let next = find(src, head, i + 1);
                if next.0 > l {
                    b.lit(src[i] as u32);
                    i += 1;
                    pending = Some(next);
                    continue;
                }
            }
            let li = (0..29).rev().find(|&k| LEN_BASE[k] as usize <= l).unwrap_or(0);
            b.lit(257 + li as u32);
            b.put((l - LEN_BASE[li] as usize) as u32, LEN_EXTRA[li] as u32);
            let di = (0..30).rev().find(|&k| DIST_BASE[k] as usize <= d).unwrap_or(0);
            b.code(di as u32, 5);
            b.put((d - DIST_BASE[di] as usize) as u32, DIST_EXTRA[di] as u32);
            for p in i + 1..i + l {
                if p + 3 <= n {
                    head[hash3(src, p)] = p as u16;
                }
            }
            i += l;
        } else {
            b.lit(src[i] as u32);
            i += 1;
        }
        if b.full {
            return None;
        }
    }
    b.lit(256);
    if b.n > 0 {
        b.put(0, 8 - b.n);
    }
    if b.full { None } else { Some(b.o) }
}

/// BACKUP-9.6: a chunk coded into the first frame of `z`, the smallest of: [`Z_ERASED`] (every byte 0xFF, the mode byte alone),
/// [`Z_DEFLATE`] and [`Z_RAW`]. Returns the coded length.
pub fn zcode(chunk: &[u8], z: &mut ZWork) -> usize {
    zcode_into(chunk, &mut z.frames[0][HDR..HDR + ZBUF], &mut z.head)
}

/// [`zcode`] into `out` (at least the chunk's length plus one).
pub fn zcode_into(chunk: &[u8], out: &mut [u8], head: &mut [u16; 1 << DEFLATE_HASH_BITS]) -> usize {
    if chunk.iter().all(|&b| b == 0xFF) {
        out[0] = Z_ERASED;
        return 1;
    }
    let (mode, rest) = out.split_at_mut(1);
    if let Some(k) = deflate_fixed(chunk, &mut rest[..chunk.len() - 1], head) {
        mode[0] = Z_DEFLATE;
        return 1 + k;
    }
    mode[0] = Z_RAW;
    rest[..chunk.len()].copy_from_slice(chunk);
    1 + chunk.len()
}

/// After a resent frame's ACK, how long a second ACK may still be on its way (FerriteFW/console#90).
pub const DUP_ACK_MS: u32 = 100;

/// A frame's ACK came on try `t`. After a resend the host may hold two copies (a NAK mid-frame, then another from
/// the first copy's tail) and ACK each, the second as a duplicate: that one is dropped here, or it would count for
/// the next frame before the host has it. Always true.
fn acked(link: &mut impl Link, t: u32) -> bool {
    if t > 0 {
        link.pause(DUP_ACK_MS);
        link.clear_rx();
    }
    true
}

/// Sends one frame until the host ACKs it: Some(true) ACK, Some(false) cancelled, None out of tries.
fn send_frame(link: &mut impl Link, hdr: &[u8; HDR], data: &[u8], resends: &mut u32) -> Option<bool> {
    let crc = frame_crc(hdr, data);
    for t in 0..TRIES {
        if t > 0 {
            *resends += 1;
        }
        link.clear_rx();
        link.write(hdr);
        link.write(data);
        link.write(&crc);
        match link.getc(ACK_MS) {
            Some(ACK) => return Some(acked(link, t)),
            Some(CAN) => return Some(false),
            _ => {}
        }
    }
    None
}

/// `fdump`'s binary part, after the "ready" line and the switch to its rate: the sync, every block of
/// `addr..addr + len` (`read` fills each from the SPI flash, read only), the end frame. `progress` gets the bytes
/// acknowledged and the blocks so far after each block, and stops the dump by returning false (the power switch
/// turned off). hwtest's `fdump`, step for step.
pub fn fdump_run(
    link: &mut impl Link,
    buf: &mut [u8; BLOCK],
    addr: u32,
    len: u32,
    read: impl FnMut(u32, &mut [u8]),
    progress: impl FnMut(u32, u32) -> bool,
) -> Result<Dumped, Stopped> {
    dump(link, buf, addr, len, read, progress)
}

/// `fdumpz` (BACKUP-9.6): `fdump` with [`ZCHUNK`] chunks sent coded ([`zcode`]) as [`TYPE_ZBLOCK`] frames; the
/// frames' CRC covers what goes over the cable, the end frame's CRC-32 the range as read, as `fdump`'s.
pub fn fdumpz_run(
    link: &mut impl Link,
    buf: &mut [u8; ZCHUNK],
    z: &mut ZWork,
    addr: u32,
    len: u32,
    mut read: impl FnMut(u32, &mut [u8]),
    mut progress: impl FnMut(u32, u32) -> bool,
) -> Result<Dumped, Stopped> {
    let t0 = link.ms();
    handshake(link, addr, t0)?;
    let (mut done, mut resends, mut crc, mut blocks) = (0u32, 0u32, 0u32, 0u32);
    let ZWork { frames, head } = z;
    let (fa, fb) = frames.split_at_mut(1);
    let f = [&mut fa[0], &mut fb[0]];
    if len > 0 {
        // Chunk N goes out from one frame while chunk N+1 is read and coded into the other; N's frame is left
        // alone until its ACK, so a resend sends it as it was.
        let mut n = len.min(ZCHUNK as u32);
        read(addr, &mut buf[..n as usize]);
        crc = crc32(crc, &buf[..n as usize]);
        let mut cur = 0;
        let mut c = zframe(f[0], addr, &buf[..n as usize], head);
        loop {
            link.clear_rx();
            link.send(&f[cur][..c]);
            let next = done + n;
            let (mut nn, mut nc) = (0, 0);
            if next < len {
                nn = (len - next).min(ZCHUNK as u32);
                read(addr + next, &mut buf[..nn as usize]);
                crc = crc32(crc, &buf[..nn as usize]);
                nc = zframe(f[1 - cur], addr + next, &buf[..nn as usize], head);
            }
            link.sent();
            match ack_or_resend(link, &f[cur][..c], &mut resends) {
                Some(true) => {}
                Some(false) => return Err(Stopped { at: addr + done, why: "cancelled by the host" }),
                None => return Err(Stopped { at: addr + done, why: "no acknowledgement" }),
            }
            done = next;
            blocks += 1;
            if !progress(done, blocks) {
                return Err(Stopped { at: addr + done, why: "the power switch is off" });
            }
            if done >= len {
                break;
            }
            cur = 1 - cur;
            n = nn;
            c = nc;
        }
    }
    let end = end_payload(addr, len, crc, resends);
    if send_frame(link, &header(TYPE_END, addr, 16), &end, &mut resends) != Some(true) {
        return Err(Stopped { at: addr + done, why: "end frame not acknowledged" });
    }
    Ok(Dumped { crc, blocks, resends, ms: link.ms().wrapping_sub(t0) })
}

/// A chunk coded into a whole frame (header, coded chunk, CRC); its length.
fn zframe(frame: &mut [u8; ZFRAME], at: u32, data: &[u8], head: &mut [u16; 1 << DEFLATE_HASH_BITS]) -> usize {
    let c = zcode_into(data, &mut frame[HDR..HDR + ZBUF], head);
    frame[..HDR].copy_from_slice(&header(TYPE_ZBLOCK, at, c as u16));
    let crc = frame_crc(&frame[..HDR], &frame[HDR..HDR + c]);
    frame[HDR + c..HDR + c + 4].copy_from_slice(&crc);
    HDR + c + 4
}

/// The ACK for a frame already sent, sending it again (whole, as it was) on a NAK or silence, as [`send_frame`].
fn ack_or_resend(link: &mut impl Link, frame: &[u8], resends: &mut u32) -> Option<bool> {
    for t in 0..TRIES {
        if t > 0 {
            *resends += 1;
            link.clear_rx();
            link.send(frame);
            link.sent();
        }
        match link.getc(ACK_MS) {
            Some(ACK) => return Some(acked(link, t)),
            Some(CAN) => return Some(false),
            _ => {}
        }
    }
    None
}

/// The sync before the first frame: the host's FDS1 until FDOK.
fn handshake(link: &mut impl Link, addr: u32, t0: u32) -> Result<(), Stopped> {
    let mut sync = Sync::default();
    let mut synced = false;
    while !synced && link.ms().wrapping_sub(t0) < SYNC_MS {
        if let Some(b) = link.getc(10) {
            synced = sync.feed(b);
        }
    }
    if !synced {
        return Err(Stopped { at: addr, why: "no sync from the host" });
    }
    // Any of the host's repeated FDS1 still on the way is dropped before the first frame.
    link.pause(20);
    link.clear_rx();
    link.write(&SYNC_OK);
    link.pause(20);
    link.clear_rx();
    Ok(())
}

fn dump(
    link: &mut impl Link,
    buf: &mut [u8],
    addr: u32,
    len: u32,
    mut read: impl FnMut(u32, &mut [u8]),
    mut progress: impl FnMut(u32, u32) -> bool,
) -> Result<Dumped, Stopped> {
    let t0 = link.ms();
    handshake(link, addr, t0)?;
    let (mut done, mut resends, mut crc, mut blocks) = (0u32, 0u32, 0u32, 0u32);
    while done < len {
        let n = (len - done).min(buf.len() as u32);
        let data = &mut buf[..n as usize];
        read(addr + done, data);
        match send_frame(link, &header(TYPE_BLOCK, addr + done, n as u16), data, &mut resends) {
            Some(true) => {}
            Some(false) => return Err(Stopped { at: addr + done, why: "cancelled by the host" }),
            None => return Err(Stopped { at: addr + done, why: "no acknowledgement" }),
        }
        crc = crc32(crc, data);
        done += n;
        blocks += 1;
        if !progress(done, blocks) {
            return Err(Stopped { at: addr + done, why: "the power switch is off" });
        }
    }
    let end = end_payload(addr, len, crc, resends);
    if send_frame(link, &header(TYPE_END, addr, 16), &end, &mut resends) != Some(true) {
        return Err(Stopped { at: addr + done, why: "end frame not acknowledged" });
    }
    Ok(Dumped { crc, blocks, resends, ms: link.ms().wrapping_sub(t0) })
}

/// Whole percent, 100 only at the end.
pub fn percent(done: u32, total: u32) -> u32 {
    if total == 0 || done >= total { 100 } else { (done as u64 * 100 / total as u64) as u32 }
}

// ---- the console ----

/// Typed bytes into a line, as hwtest's `cmdline_feed`.
pub struct LineEdit {
    buf: [u8; 64],
    len: usize,
}

/// What one byte did, and what to echo for it.
#[derive(Debug, PartialEq)]
pub enum Fed {
    Nothing,
    /// Echo this character.
    Echo(u8),
    /// Echo "\b \b".
    Erase,
    /// Echo "\r\n": the line is complete ([`LineEdit::line`]).
    Line,
}

impl Default for LineEdit {
    fn default() -> Self {
        LineEdit { buf: [0; 64], len: 0 }
    }
}

impl LineEdit {
    pub fn feed(&mut self, c: u8) -> Fed {
        match c {
            // The \n of a \r\n pair (or a bare one) after an empty line: nothing.
            b'\n' if self.len == 0 => Fed::Nothing,
            b'\r' | b'\n' => Fed::Line,
            0x08 | 0x7F if self.len > 0 => {
                self.len -= 1;
                Fed::Erase
            }
            0x20..=0x7E if self.len < self.buf.len() - 1 => {
                self.buf[self.len] = c;
                self.len += 1;
                Fed::Echo(c)
            }
            _ => Fed::Nothing,
        }
    }

    pub fn line(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// hwtest's `cmd_uint`: decimal, or hex after `0x`.
pub fn uint(s: &[u8]) -> Option<u32> {
    let (base, digits) = match s {
        [b'0', b'x' | b'X', rest @ ..] => (16, rest),
        _ => (10, s),
    };
    if digits.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in digits {
        let d = (c as char).to_digit(base)?;
        v = v.checked_mul(base)?.checked_add(d)?;
    }
    Some(v)
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Empty,
    Help,
    Sys,
    Flash,
    FlashCrc,
    Fdump { addr: u32, len: u32, baud: u32 },
    /// `fdumpz`: as `fdump`, coded (BACKUP-9.6).
    FdumpZ { addr: u32, len: u32, baud: u32 },
    /// `mdump <addr> <len>`: the MCU's flash, read only (hwtest's command).
    Mdump { addr: u32, len: u32 },
    Reader,
    /// `tune <pace bytes> <pace us> <stop bits>`: the dump's pacing and stop bits, for link tests.
    Tune { bytes: u32, us: u32, stop2: bool },
    Reboot,
    Off,
    Unknown,
}

/// A console line as a command; what hwtest wouldn't run is [`Command::Unknown`].
pub fn parse(line: &[u8]) -> Command {
    let mut words = line.split(|&c| c == b' ' || c == b'\t').filter(|w| !w.is_empty());
    let Some(cmd) = words.next() else { return Command::Empty };
    let a: [Option<&[u8]>; 3] = [words.next(), words.next(), words.next()];
    match cmd {
        b"help" | b"?" => Command::Help,
        b"sys" => Command::Sys,
        b"flash" => Command::Flash,
        b"flashcrc" => Command::FlashCrc,
        b"reader" => Command::Reader,
        b"tune" => match (a[0].and_then(uint), a[1].and_then(uint), a[2].and_then(uint)) {
            (Some(bytes), Some(us), Some(stop)) => Command::Tune { bytes, us, stop2: stop == 2 },
            _ => Command::Unknown,
        },
        b"reboot" => Command::Reboot,
        b"off" => Command::Off,
        b"mdump" => match (a[0].and_then(uint), a[1].and_then(uint)) {
            (Some(addr), Some(len)) => Command::Mdump { addr, len },
            _ => Command::Unknown,
        },
        b"fdump" | b"fdumpz" => {
            let (Some(addr), Some(len)) = (a[0].and_then(uint), a[1].and_then(uint)) else { return Command::Unknown };
            let baud = match a[2] {
                None if cmd == b"fdumpz" => FDUMPZ_BAUD,
                None => FDUMP_BAUD,
                Some(b) => match uint(b) {
                    Some(b) => b,
                    None => return Command::Empty, // hwtest: a bad rate is ignored silently
                },
            };
            if cmd == b"fdumpz" { Command::FdumpZ { addr, len, baud } } else { Command::Fdump { addr, len, baud } }
        }
        _ => Command::Unknown,
    }
}

// ---- the answers ----

/// Where a line goes (the cable on the radio, a Vec in the tests).
pub trait Out {
    fn put(&mut self, b: &[u8]);
}

/// Upper-case hex, `digits` at least.
pub fn hex(o: &mut impl Out, v: u32, digits: usize) {
    let mut s = [0u8; 8];
    let mut n = 0;
    let mut x = v;
    while n < 8 && (x != 0 || n < digits.max(1)) {
        s[7 - n] = b"0123456789ABCDEF"[(x & 15) as usize];
        x >>= 4;
        n += 1;
    }
    o.put(&s[8 - n..]);
}

pub fn dec(o: &mut impl Out, v: u32) {
    let mut s = [0u8; 10];
    let mut n = 0;
    let mut x = v;
    loop {
        s[9 - n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
        if x == 0 {
            break;
        }
    }
    o.put(&s[10 - n..]);
}

pub const NL: &[u8] = b"\r\n";

/// `sys`: the image, the clock and the MCU, then ` uid ` and the 96-bit unique ID (three words, lowest first).
pub fn sys_line(o: &mut impl Out, version: &str, mhz: u32, idcode: u32, flash_kb: u32, uid: [u32; 3]) {
    o.put(b"Ferrite reader ");
    o.put(version.as_bytes());
    o.put(b"  clock ");
    dec(o, mhz);
    o.put(b" MHz  mcu idcode ");
    hex(o, idcode, 8);
    o.put(b"  flash ");
    dec(o, flash_kb);
    o.put(b" KB  uid ");
    for w in uid {
        hex(o, w, 8);
    }
    o.put(NL);
}

/// What `flash` reports about the chip.
pub struct FlashInfo {
    pub jedec: u32,
    pub status: u8,
    pub uid: [u8; 8],
    /// The byte sum and blankness of the calibration sector (0x00F000-0x00FFFF), read only.
    pub cal_sum: u32,
    pub cal_blank: bool,
}

/// `flash`: hwtest's line; Console takes the ID, vendor and size from it.
pub fn flash_line(o: &mut impl Out, f: &FlashInfo) {
    o.put(b"flash id ");
    hex(o, f.jedec, 6);
    o.put(b" (");
    o.put(crate::chipid::flash_vendor(f.jedec).as_bytes());
    o.put(b", ");
    dec(o, crate::chipid::flash_bytes(f.jedec) / 1024);
    o.put(b" KB) status ");
    hex(o, f.status as u32, 2);
    o.put(b" uid ");
    for b in f.uid {
        hex(o, b as u32, 2);
    }
    o.put(b"  cal sector sum ");
    hex(o, f.cal_sum, 8);
    if f.cal_blank {
        o.put(b" (blank)");
    }
    o.put(NL);
}

/// `flashcrc`'s result.
pub fn flashcrc_line(o: &mut impl Out, crc: u32, bytes: u32, ms: u32, busy_ms: u32) {
    o.put(b"flash crc32 ");
    hex(o, crc, 8);
    o.put(b" over ");
    dec(o, bytes / 1024);
    o.put(b" KB, ");
    dec(o, ms / 1000);
    o.put(b" s (read + CRC ");
    dec(o, ((bytes / 1024) * 1000).checked_div(busy_ms).unwrap_or(0));
    o.put(b" KB/s)");
    o.put(NL);
}

/// Why an `fdump` won't run, before anything changes.
pub fn fdump_refusal(o: &mut impl Out, addr: u32, len: u32, chip: u32) -> bool {
    if chip == 0 || addr >= chip || len == 0 || len > chip - addr {
        o.put(b"fdump: range ");
        hex(o, addr, 6);
        o.put(b"+");
        hex(o, len, 1);
        o.put(b" outside the ");
        dec(o, chip / 1024);
        o.put(b" KB chip");
        o.put(NL);
        return true;
    }
    false
}

/// Why an `mdump` won't run: outside the MCU's main flash (`flash_kb`: the chip's size register, 1024 at most), the
/// only memory it reads. hwtest's line, word for word.
pub fn mdump_refusal(o: &mut impl Out, addr: u32, len: u32, flash_kb: u32) -> bool {
    let end = MCU_FLASH + flash_kb.min(1024) * 1024;
    if flash_kb == 0 || addr < MCU_FLASH || addr >= end || len > end - addr {
        o.put(b"mdump: 0x08000000-0x");
        hex(o, end.wrapping_sub(1), 8);
        o.put(b" only");
        o.put(NL);
        return true;
    }
    false
}

/// One `mdump` line: `80F0000:` + the bytes in hex + `*` + their byte sum.
pub fn mdump_line(o: &mut impl Out, addr: u32, bytes: &[u8]) {
    hex(o, addr, 6);
    o.put(b":");
    let mut sum = 0u8;
    for &b in bytes {
        hex(o, b as u32, 2);
        sum = sum.wrapping_add(b);
    }
    o.put(b"*");
    hex(o, sum as u32, 2);
    o.put(NL);
}

pub fn mdump_end(o: &mut impl Out) {
    o.put(b"dump: end");
    o.put(NL);
}

pub fn fdump_bad_rate(o: &mut impl Out, baud: u32, pclk: u32) {
    o.put(b"fdump: ");
    dec(o, baud);
    o.put(b" baud not within 2% from ");
    dec(o, pclk);
    o.put(b" Hz (500000, 250000, 115200 are exact)");
    o.put(NL);
}

pub fn fdump_ready(o: &mut impl Out, addr: u32, len: u32, baud: u32, mhz: u32) {
    o.put(b"fdump ");
    hex(o, addr, 6);
    o.put(b" ");
    hex(o, len, 1);
    o.put(b" ");
    dec(o, baud);
    o.put(b" ready, clock ");
    dec(o, mhz);
    o.put(b" MHz");
    o.put(NL);
}

pub fn fdump_end(o: &mut impl Out, crc: u32, len: u32, blocks: u32, resends: u32, ms: u32) {
    o.put(b"\r\nfdump end crc32 ");
    hex(o, crc, 8);
    o.put(b", ");
    dec(o, len);
    o.put(b" bytes in ");
    dec(o, blocks);
    o.put(b" blocks, ");
    dec(o, resends);
    o.put(b" resends, ");
    dec(o, ms);
    o.put(b" ms");
    o.put(NL);
}

pub fn fdump_stopped(o: &mut impl Out, at: u32, why: &str) {
    o.put(b"\r\nfdump stopped at ");
    hex(o, at, 6);
    o.put(b": ");
    o.put(why.as_bytes());
    o.put(b"; back to ");
    dec(o, CONSOLE_BAUD);
    o.put(b" baud");
    o.put(NL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    impl Out for Vec<u8> {
        fn put(&mut self, b: &[u8]) {
            self.extend_from_slice(b)
        }
    }

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut v = Vec::new();
        f(&mut v);
        String::from_utf8(v).unwrap()
    }

    /// BACKUP-12: `mdump` as hwtest's (fw/apps/hwtest/svc.c `dump`): the marker sector's line, the refusal outside
    /// the main flash, and the command parsed as hwtest's.
    #[test]
    fn mdump_reads_the_marker_as_hwtest() {
        assert_eq!(parse(b"mdump 0x080F0000 16"), Command::Mdump { addr: MARKER_ADDR, len: MARKER_LEN });
        assert_eq!(parse(b"mdump 0x080F0000"), Command::Unknown);
        let mut m = [0xFFu8; 16];
        m[..4].copy_from_slice(b"UV85");
        assert_eq!(
            text(|o| mdump_line(o, MARKER_ADDR, &m)),
            "80F0000:55563835FFFFFFFFFFFFFFFFFFFFFFFF*0C\r\n"
        );
        assert_eq!(text(|o| mdump_line(o, 0x0800_0000, &[0x12])), "8000000:12*12\r\n");
        assert_eq!(text(mdump_end), "dump: end\r\n");
        assert!(!mdump_refusal(&mut Vec::new(), MARKER_ADDR, MARKER_LEN, 1024));
        assert!(!mdump_refusal(&mut Vec::new(), 0x080F_FFF0, 16, 1024));
        for (addr, len, kb) in [(0x080F_FFF0, 17, 1024), (0x0810_0000, 1, 1024), (0x07FF_FFFF, 1, 1024), (0x2000_0000, 4, 1024), (0x1FFF_F800, 48, 1024), (MARKER_ADDR, 16, 512), (MARKER_ADDR, 16, 0), (0x0800_0000, u32::MAX, 1024)] {
            let mut o = Vec::new();
            assert!(mdump_refusal(&mut o, addr, len, kb), "{addr:#x}+{len} on {kb} KB");
            if kb == 1024 {
                assert_eq!(String::from_utf8(o).unwrap(), "mdump: 0x08000000-0x080FFFFF only\r\n");
            }
        }
        assert!(String::from_utf8(HELP.to_vec()).unwrap().contains("mdump <addr> <len>"));
    }

    /// BACKUP-9.5: the reader's CRC is zlib's, continued across pieces, as hwtest's `crc32_update` and Console's.
    #[test]
    fn crc32_is_zlibs_and_continues() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(0, b""), 0);
        let data: Vec<u8> = (0..10_000u32).map(|i| (i * 31 + 7) as u8).collect();
        let whole = crc32(0, &data);
        let mut c = 0;
        for piece in data.chunks(256) {
            c = crc32(c, piece);
        }
        assert_eq!(c, whole);
        // A blank 4 MB chip, as `flashcrc` would report it.
        let mut c = 0;
        let ff = [0xFFu8; 4096];
        for _ in 0..1024 {
            c = crc32(c, &ff);
        }
        assert_eq!(c, crc32(0, &std::vec![0xFFu8; 4 << 20]));
    }

    /// BACKUP-9.5: frames as fdump.h builds them (header LE, CRC over header and data, the end payload).
    #[test]
    fn frames_match_fdump_h() {
        let h = header(TYPE_BLOCK, 0x0012_3456, 0x1000);
        assert_eq!(h, [b'F', b'D', b'B', 0, 0x56, 0x34, 0x12, 0x00, 0x00, 0x10]);
        let data = [1u8, 2, 3];
        let mut all = h.to_vec();
        all.extend_from_slice(&data);
        assert_eq!(frame_crc(&h, &data), crc32(0, &all).to_le_bytes());
        let e = end_payload(0, 0x40_0000, 0xDEAD_BEEF, 3);
        assert_eq!(&e[..4], &[0, 0, 0, 0]);
        assert_eq!(&e[4..8], &[0, 0, 0x40, 0]);
        assert_eq!(&e[8..12], &[0xEF, 0xBE, 0xAD, 0xDE]);
        assert_eq!(&e[12..], &[3, 0, 0, 0]);
        assert_eq!(header(TYPE_END, 0, 16), [b'F', b'D', b'E', 0, 0, 0, 0, 0, 16, 0]);
    }

    /// BACKUP-9.5: the host's FDS1 is found after noise, repeated, or split oddly.
    #[test]
    fn sync_after_noise() {
        let mut s = Sync::default();
        let hits: Vec<bool> = b"\x00xFDFFDS1".iter().map(|&b| s.feed(b)).collect();
        assert_eq!(hits.iter().filter(|&&h| h).count(), 1);
        assert!(*hits.last().unwrap());
        let mut s = Sync::default();
        assert!(!b"FDS".iter().any(|&b| s.feed(b)));
        assert!(!s.feed(b'2'));
        assert!(!b"FDS".iter().any(|&b| s.feed(b)));
        assert!(s.feed(b'1'));
    }

    /// BACKUP-9.5: the commands Console and backup.py send, as hwtest parsed them.
    #[test]
    fn commands_parse_like_hwtest() {
        assert_eq!(parse(b"fdump 0x0 0x400000 500000"), Command::Fdump { addr: 0, len: 0x40_0000, baud: 500_000 });
        assert_eq!(parse(b"fdump 0xF000 4096"), Command::Fdump { addr: 0xF000, len: 4096, baud: FDUMP_BAUD });
        assert_eq!(parse(b"  fdump  0X10 0x20  115200 "), Command::Fdump { addr: 0x10, len: 0x20, baud: 115_200 });
        assert_eq!(parse(b"fdump 0x10"), Command::Unknown);
        assert_eq!(parse(b"fdump 0xG 1"), Command::Unknown);
        assert_eq!(parse(b"sys"), Command::Sys);
        assert_eq!(parse(b"flash"), Command::Flash);
        assert_eq!(parse(b"flashcrc"), Command::FlashCrc);
        assert_eq!(parse(b"reader"), Command::Reader);
        assert_eq!(parse(b"reboot"), Command::Reboot);
        assert_eq!(parse(b""), Command::Empty);
        assert_eq!(parse(b"write 0 0"), Command::Unknown);
        assert_eq!(uint(b"4294967295"), Some(u32::MAX));
        assert_eq!(uint(b"4294967296"), None);
        assert_eq!(uint(b"0x"), None);
        assert_eq!(uint(b"0xffffFFFF"), Some(u32::MAX));
    }

    /// BACKUP-9.5: typed bytes echo as hwtest's console did; CR LF ends a line once.
    #[test]
    fn line_editing() {
        let mut e = LineEdit::default();
        assert_eq!(e.feed(b'\n'), Fed::Nothing);
        assert_eq!(e.feed(b's'), Fed::Echo(b's'));
        assert_eq!(e.feed(b'x'), Fed::Echo(b'x'));
        assert_eq!(e.feed(0x7F), Fed::Erase);
        assert_eq!(e.feed(0x01), Fed::Nothing);
        assert_eq!(e.feed(b'y'), Fed::Echo(b'y'));
        assert_eq!(e.feed(b'\r'), Fed::Line);
        assert_eq!(e.line(), b"sy");
        e.clear();
        assert_eq!(e.feed(b'\n'), Fed::Nothing);
        assert_eq!(e.feed(b'\r'), Fed::Line);
        assert_eq!(e.line(), b"");
        e.clear();
        for _ in 0..100 {
            e.feed(b'a');
        }
        assert_eq!(e.line().len(), 63);
    }

    /// BACKUP-9.4, BACKUP-9.5: the lines Console reads, in hwtest's format (Console's patterns: ` uid ` and 24 hex
    /// digits; `flash id XXXXXX (vendor, N KB)`; `flash crc32 XXXXXXXX over N KB`; `fdump XXXXXX X N ready`).
    #[test]
    fn answers_are_hwtests_lines() {
        let s = text(|o| sys_line(o, "v0.1.0", 120, 0x7005_0344, 1024, [0x3F2A_91C0, 0x7D44_0123, 0x89AB_CDEF]));
        assert_eq!(s, "Ferrite reader v0.1.0  clock 120 MHz  mcu idcode 70050344  flash 1024 KB  uid 3F2A91C07D44012389ABCDEF\r\n");
        assert!(!s.contains("Ferrite terminal"));
        let s = text(|o| sys_line(o, "v0.1.0", 120, 0, 0, [1, 0, 0xA]));
        assert!(s.ends_with(" uid 00000001000000000000000A\r\n"));
        let f = FlashInfo { jedec: 0xEF4016, status: 0, uid: [0xA0, 1, 2, 3, 4, 5, 6, 0xFF], cal_sum: 0x1234, cal_blank: false };
        assert_eq!(
            text(|o| flash_line(o, &f)),
            "flash id EF4016 (Winbond, 4096 KB) status 00 uid A0010203040506FF  cal sector sum 00001234\r\n"
        );
        let f = FlashInfo { jedec: 0x5E4016, status: 0x1C, cal_blank: true, ..f };
        assert!(text(|o| flash_line(o, &f)).starts_with("flash id 5E4016 (Zbit, 4096 KB) status 1C "));
        assert!(text(|o| flash_line(o, &f)).ends_with(" (blank)\r\n"));
        assert_eq!(
            text(|o| flashcrc_line(o, 0x1A2B_3C4D, 4 << 20, 52_300, 4_000)),
            "flash crc32 1A2B3C4D over 4096 KB, 52 s (read + CRC 1024 KB/s)\r\n"
        );
        assert_eq!(text(|o| fdump_ready(o, 0, 0x40_0000, 500_000, 120)), "fdump 000000 400000 500000 ready, clock 120 MHz\r\n");
        assert_eq!(text(|o| fdump_ready(o, 0xF000, 0x1000, 250_000, 120)), "fdump 00F000 1000 250000 ready, clock 120 MHz\r\n");
        assert_eq!(
            text(|o| fdump_end(o, 0xCBF4_3926, 9, 1, 0, 12)),
            "\r\nfdump end crc32 CBF43926, 9 bytes in 1 blocks, 0 resends, 12 ms\r\n"
        );
        assert_eq!(
            text(|o| fdump_stopped(o, 0x1000, "cancelled by the host")),
            "\r\nfdump stopped at 001000: cancelled by the host; back to 115200 baud\r\n"
        );
        assert!(String::from_utf8_lossy(HELP).contains("fdump <addr> <len>"));
    }

    /// A host as Console's `Fdump` is: FDS1 until FDOK, each frame checked and answered. `bad` frames (by number,
    /// counting resends) are answered NAK, `silent` ones not at all (a lost ACK), `cancel_at` with CAN.
    struct Host {
        to_radio: std::collections::VecDeque<u8>,
        from_radio: Vec<u8>,
        synced: bool,
        frames: u32,
        bad: Vec<u32>,
        silent: Vec<u32>,
        cancel_at: Option<u32>,
        data: Vec<u8>,
        end: Option<[u8; 16]>,
        now: u32,
        clears: u32,
        /// fdumpz: bytes still to come (an erased chunk carries no length of its own).
        zlen: usize,
        /// Frames (by number) the host ACKs twice, as it does when it got two copies (FerriteFW/console#90): the
        /// second ACK is `late`, arriving after the radio's next clear, while it waits or sends the next frame.
        dup_ack: Vec<u32>,
        late: Vec<u8>,
    }

    impl Host {
        fn new() -> Self {
            Host {
                to_radio: b"\x00noiseFDS1FDS1".iter().copied().collect(),
                from_radio: Vec::new(),
                synced: false,
                frames: 0,
                bad: Vec::new(),
                silent: Vec::new(),
                cancel_at: None,
                data: Vec::new(),
                end: None,
                now: 0,
                clears: 0,
                zlen: 0,
                dup_ack: Vec::new(),
                late: Vec::new(),
            }
        }

        /// A whole frame from the radio: checked and answered.
        fn take_frame(&mut self) {
            let b = &self.from_radio;
            if b.len() < HDR {
                return;
            }
            let n = u16::from_le_bytes([b[8], b[9]]) as usize;
            if b.len() < HDR + n + 4 {
                return;
            }
            let frame: Vec<u8> = self.from_radio.drain(..HDR + n + 4).collect();
            let k = self.frames;
            self.frames += 1;
            assert_eq!(&frame[..2], b"FD");
            let ok = frame_crc(frame[..HDR].try_into().unwrap(), &frame[HDR..HDR + n]) == frame[HDR + n..];
            assert!(ok, "frame {k}: bad CRC");
            if self.cancel_at == Some(k) {
                return self.to_radio.push_back(CAN);
            }
            if self.silent.contains(&k) {
                return;
            }
            if self.bad.contains(&k) {
                return self.to_radio.push_back(NAK);
            }
            match frame[2] {
                TYPE_BLOCK => {
                    let a = u32::from_le_bytes(frame[4..8].try_into().unwrap());
                    assert_eq!(a as usize, self.data.len(), "blocks in order, no repeats");
                    self.data.extend_from_slice(&frame[HDR..HDR + n]);
                }
                TYPE_ZBLOCK => {
                    let a = u32::from_le_bytes(frame[4..8].try_into().unwrap());
                    assert_eq!(a as usize, self.data.len(), "chunks in order, no repeats");
                    let (mode, body) = (frame[HDR], &frame[HDR + 1..HDR + n]);
                    let chunk = match mode {
                        Z_RAW => body.to_vec(),
                        Z_DEFLATE => miniz_oxide::inflate::decompress_to_vec(body).unwrap(),
                        Z_ERASED => std::vec![0xFF; self.zlen.min(ZCHUNK)],
                        m => panic!("mode {m}"),
                    };
                    self.zlen -= chunk.len();
                    self.data.extend_from_slice(&chunk);
                }
                TYPE_END => self.end = Some(frame[HDR..HDR + 16].try_into().unwrap()),
                t => panic!("frame type {t}"),
            }
            self.to_radio.push_back(ACK);
            if self.dup_ack.contains(&k) {
                self.late.push(ACK);
            }
        }
    }

    impl Link for Host {
        fn write(&mut self, b: &[u8]) {
            if !self.synced {
                assert_eq!(b, b"FDOK");
                self.synced = true;
                return;
            }
            self.to_radio.extend(self.late.drain(..));
            self.from_radio.extend_from_slice(b);
            self.take_frame();
        }
        fn getc(&mut self, ms: u32) -> Option<u8> {
            let b = self.to_radio.pop_front();
            if b.is_none() {
                self.now += ms;
            }
            b
        }
        fn clear_rx(&mut self) {
            self.clears += 1;
            if self.synced {
                self.to_radio.clear();
            }
        }
        fn ms(&mut self) -> u32 {
            self.now
        }
        fn pause(&mut self, ms: u32) {
            self.now += ms;
            self.to_radio.extend(self.late.drain(..));
        }
    }

    fn chip(a: u32) -> u8 {
        (a.wrapping_mul(2_654_435_761) >> 13) as u8
    }

    fn run(host: &mut Host, addr: u32, len: u32) -> Result<Dumped, Stopped> {
        let mut buf = [0u8; BLOCK];
        let mut last = 0;
        fdump_run(
            host,
            &mut buf,
            addr,
            len,
            |a, d| d.iter_mut().enumerate().for_each(|(i, x)| *x = chip(a + i as u32)),
            |done, _| {
                assert!(done > last);
                last = done;
                true
            },
        )
    }

    /// BACKUP-9.6: every coding round-trips, picks erased/RLE/raw by size, and never beats raw's room.
    #[test]
    fn zcode_round_trips() {
        let mut text = [0u8; ZCHUNK];
        for (i, b) in text.iter_mut().enumerate() {
            *b = b"CQ CQ de VA7XYZ 146.520 FM "[i % 27] ^ ((i / 997) as u8 & 1);
        }
        let mut cases: [[u8; ZCHUNK]; 6] = [[0xFF; ZCHUNK], [0; ZCHUNK], [0; ZCHUNK], [0; ZCHUNK], [0xFF; ZCHUNK], text];
        let mut x = 0x2545_F491u32;
        for b in cases[2].iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = x as u8; // noise: raw
        }
        for (i, b) in cases[3].iter_mut().enumerate() {
            *b = if i % 300 < 200 { 0xFF } else { chip(i as u32) }; // mixed
        }
        cases[4][100] = 0x12; // one byte off erased
        let modes = [Z_ERASED, Z_DEFLATE, Z_RAW, Z_DEFLATE, Z_DEFLATE, Z_DEFLATE];
        let mut z = std::boxed::Box::new(ZWork::new());
        for (c, m) in cases.iter().zip(modes) {
            let n = zcode(c, &mut z);
            assert_eq!(z.coded()[0], m);
            assert!(n <= ZBUF);
            let back = match m {
                Z_ERASED => std::vec![0xFF; ZCHUNK],
                Z_RAW => z.coded()[1..n].to_vec(),
                _ => miniz_oxide::inflate::decompress_to_vec(&z.coded()[1..n]).unwrap(),
            };
            assert_eq!(&back[..], &c[..]);
        }
        assert!(zcode(&text, &mut z) < 600);
        // Short and odd lengths: the end of a range.
        for len in [1usize, 2, 3, 4, 5, 259, 1000] {
            let n = zcode(&cases[3][..len], &mut z);
            let back = match z.coded()[0] {
                Z_RAW => z.coded()[1..n].to_vec(),
                Z_ERASED => std::vec![0xFF; len],
                _ => miniz_oxide::inflate::decompress_to_vec(&z.coded()[1..n]).unwrap(),
            };
            assert_eq!(&back[..], &cases[3][..len]);
        }
        assert_eq!(parse(b"fdumpz 0 0x400000 460800"), Command::FdumpZ { addr: 0, len: 0x40_0000, baud: 460_800 });
        assert_eq!(parse(b"fdumpz 0 0x1000"), Command::FdumpZ { addr: 0, len: 0x1000, baud: FDUMPZ_BAUD });
    }

    /// BACKUP-9.6: the coding on a real image (FERRITE_DUMP=path; does nothing without it). Every chunk inflates back.
    #[test]
    fn zcode_on_a_real_image() {
        let Ok(path) = std::env::var("FERRITE_DUMP") else { return };
        let img = std::fs::read(path).unwrap();
        let mut z = std::boxed::Box::new(ZWork::new());
        let (mut total, mut modes) = (0usize, [0usize; 5]);
        for c in img.chunks(ZCHUNK) {
            let n = zcode(c, &mut z);
            modes[z.coded()[0] as usize] += 1;
            if z.coded()[0] == Z_DEFLATE {
                assert_eq!(miniz_oxide::inflate::decompress_to_vec(&z.coded()[1..n]).unwrap(), c);
            }
            total += n + HDR + 4;
        }
        std::println!("image {} KB -> {} KB on the cable; chunks raw {} erased {} deflate {}", img.len() / 1024, total / 1024, modes[0], modes[1], modes[4]);
    }

    /// BACKUP-9.6: every chunk of `fdumpz`, double buffered, reaches the host whole and in order.
    #[test]
    fn fdumpz_double_buffered_with_resends() {
        // Erased, compressible and noisy chunks; a NAK on the second frame and silence on the fourth: each
        // resend must be the frame as it was, though the next chunk was coded meanwhile.
        let len = 4 * ZCHUNK as u32 + 777;
        let byte = |a: u32| -> u8 {
            match a / ZCHUNK as u32 {
                0 => 0xFF,
                1 => b"FERRITE "[a as usize % 8],
                _ => chip(a),
            }
        };
        let mut h = Host::new();
        h.zlen = len as usize;
        h.bad = std::vec![1];
        h.silent = std::vec![3];
        let mut buf = std::boxed::Box::new([0u8; ZCHUNK]);
        let mut z = std::boxed::Box::new(ZWork::new());
        let d = fdumpz_run(
            &mut h,
            &mut buf,
            &mut z,
            0,
            len,
            |a, d| d.iter_mut().enumerate().for_each(|(i, x)| *x = byte(a + i as u32)),
            |_, _| true,
        )
        .unwrap();
        let want: Vec<u8> = (0..len).map(byte).collect();
        assert_eq!(h.data, want);
        assert_eq!(d.crc, crc32(0, &want));
        assert_eq!(d.resends, 2);
        assert_eq!(d.blocks, 5);
    }

    /// BACKUP-9.9: after a resend the host may ACK two copies of the frame; the second ACK is dropped, not taken
    /// for the next frame (which here the host NAKs: taking the stale ACK would skip it). FerriteFW/console#90.
    #[test]
    fn fdumpz_drops_a_duplicate_ack() {
        let len = 4 * ZCHUNK as u32;
        let mut h = Host::new();
        h.zlen = len as usize;
        h.bad = std::vec![1, 3]; // chunk 1 NAKed once, then chunk 2 once
        h.dup_ack = std::vec![2]; // chunk 1's resend ACKed twice
        let mut buf = std::boxed::Box::new([0u8; ZCHUNK]);
        let mut z = std::boxed::Box::new(ZWork::new());
        let d = fdumpz_run(&mut h, &mut buf, &mut z, 0, len, |a, d| d.iter_mut().enumerate().for_each(|(i, x)| *x = chip(a + i as u32)), |_, _| true)
            .unwrap();
        assert_eq!(h.data, (0..len).map(chip).collect::<Vec<_>>());
        assert_eq!((d.blocks, d.resends), (4, 2));
    }

    /// BACKUP-9.9: as [`fdumpz_drops_a_duplicate_ack`], for plain `fdump`.
    #[test]
    fn fdump_drops_a_duplicate_ack() {
        let mut h = Host::new();
        h.bad = std::vec![1, 3];
        h.dup_ack = std::vec![2];
        let len = 4 * BLOCK as u32;
        let r = run(&mut h, 0, len).unwrap();
        assert_eq!(h.data, (0..len).map(chip).collect::<Vec<_>>());
        assert_eq!((r.blocks, r.resends), (4, 2));
    }

    /// BACKUP-9.5: a whole dump against a host like Console's: every byte, the end frame's CRC and count.
    #[test]
    fn fdump_whole_range() {
        let mut h = Host::new();
        let len = 3 * BLOCK as u32 + 100;
        let r = run(&mut h, 0, len).unwrap();
        let want: Vec<u8> = (0..len).map(chip).collect();
        assert_eq!(h.data, want);
        assert_eq!(r.crc, crc32(0, &want));
        assert_eq!((r.blocks, r.resends), (4, 0));
        assert_eq!(h.end.unwrap(), end_payload(0, len, crc32(0, &want), 0));
    }

    /// BACKUP-9.5: a NAK and a lost ACK each resend the frame (counted), and the data is still whole and in order.
    #[test]
    fn fdump_resends() {
        let mut h = Host::new();
        h.bad = std::vec![1];
        h.silent = std::vec![2]; // block 1 sent again, and its ACK lost
        let len = 2 * BLOCK as u32;
        let r = run(&mut h, 0, len).unwrap();
        assert_eq!(h.data, (0..len).map(chip).collect::<Vec<_>>());
        assert_eq!(r.resends, 2);
        assert_eq!(u32::from_le_bytes(h.end.unwrap()[12..].try_into().unwrap()), 2);
    }

    /// BACKUP-9.5: CAN stops at once; no sync, or a host that never answers, stops with the reason.
    #[test]
    fn fdump_stops() {
        let mut h = Host::new();
        h.cancel_at = Some(1);
        assert_eq!(run(&mut h, 0, 4 * BLOCK as u32), Err(Stopped { at: BLOCK as u32, why: "cancelled by the host" }));
        let mut h = Host::new();
        h.to_radio = b"FDS2FD".iter().copied().collect();
        assert_eq!(run(&mut h, 0, 10), Err(Stopped { at: 0, why: "no sync from the host" }));
        assert!(h.now >= SYNC_MS);
        let mut h = Host::new();
        h.silent = (0..TRIES).collect();
        assert_eq!(run(&mut h, 0, 10), Err(Stopped { at: 0, why: "no acknowledgement" }));
        assert_eq!(h.frames, TRIES);
    }

    /// BACKUP-9.5: a dump past the chip is refused before the rate changes, as hwtest did.
    #[test]
    fn fdump_range_refusals() {
        let chip = 4 << 20;
        assert!(!fdump_refusal(&mut Vec::new(), 0, chip, chip));
        assert!(!fdump_refusal(&mut Vec::new(), chip - 1, 1, chip));
        for (a, l) in [(chip, 1), (0, 0), (0, chip + 1), (chip - 1, 2), (1, u32::MAX)] {
            let s = text(|o| assert!(fdump_refusal(o, a, l, chip)));
            assert!(s.starts_with("fdump: range ") && s.contains("outside the 4096 KB chip"), "{s}");
        }
        assert!(fdump_refusal(&mut Vec::new(), 0, 1, 0), "no chip, no dump");
        assert_eq!(text(|o| fdump_bad_rate(o, 7, 60_000_000)), "fdump: 7 baud not within 2% from 60000000 Hz (500000, 250000, 115200 are exact)\r\n");
    }

    #[test]
    fn percent_and_numbers() {
        assert_eq!(percent(0, 100), 0);
        assert_eq!(percent(99, 100), 99);
        assert_eq!(percent(4 << 20, 4 << 20), 100);
        assert_eq!(percent((4 << 20) - 1, 4 << 20), 99);
        assert_eq!(percent(1, 0), 100);
        assert_eq!(text(|o| hex(o, 0, 1)), "0");
        assert_eq!(text(|o| hex(o, 0xABC, 6)), "000ABC");
        assert_eq!(text(|o| hex(o, u32::MAX, 2)), "FFFFFFFF");
        assert_eq!(text(|o| dec(o, 0)), "0");
        assert_eq!(text(|o| dec(o, u32::MAX)), "4294967295");
    }

    /// The table built at compile time is zlib's (its well-known entries), the same when built at run time.
    #[test]
    fn crc_table_is_zlibs() {
        let t = table();
        assert_eq!((t[0], t[1], t[128], t[255]), (0, 0x7707_3096, 0xEDB8_8320, 0x2D02_EF8D));
        assert_eq!(t, CRC_TABLE);
    }

    /// The commands not in [`commands_parse_like_hwtest`]: help, off, the link test's `tune`, a bad rate.
    #[test]
    fn more_commands() {
        assert_eq!(parse(b"help"), Command::Help);
        assert_eq!(parse(b"?"), Command::Help);
        assert_eq!(parse(b"off"), Command::Off);
        assert_eq!(parse(b"\t "), Command::Empty);
        assert_eq!(parse(b"tune 64 500 2"), Command::Tune { bytes: 64, us: 500, stop2: true });
        assert_eq!(parse(b"tune 0x40 0 1"), Command::Tune { bytes: 64, us: 0, stop2: false });
        assert_eq!(parse(b"tune 64 500"), Command::Unknown);
        assert_eq!(parse(b"tune 64 x 2"), Command::Unknown);
        assert_eq!(parse(b"fdump 0 16 fast"), Command::Empty, "hwtest ignores a bad rate silently");
        assert_eq!(parse(b"fdumpz 0 16 0x"), Command::Empty);
        assert_eq!(uint(b"99999999999"), None, "overflow on the multiply");
        assert_eq!(uint(b"0x1G"), None);
    }

    /// BACKUP-9.6: a coded chunk never needs more than raw; DEFLATE gives up (None) when its room is too small.
    #[test]
    fn deflate_gives_up_when_full() {
        let mut head = std::boxed::Box::new([0u16; 1 << DEFLATE_HASH_BITS]);
        let noise: Vec<u8> = (0..200u32).map(chip).collect();
        assert_eq!(deflate_fixed(&noise, &mut [0u8; 16], &mut head), None);
        let n = deflate_fixed(&noise, &mut [0u8; 400], &mut head).unwrap();
        assert!(n > 16 && n < 400);
        // A long run fits in a few bytes, and no room at all is still None.
        let mut out = [0u8; 16];
        let k = deflate_fixed(&[0x41; 1000], &mut out, &mut head).unwrap();
        assert_eq!(miniz_oxide::inflate::decompress_to_vec(&out[..k]).unwrap(), [0x41; 1000]);
        assert_eq!(deflate_fixed(&[0x41; 1000], &mut [], &mut head), None);
        let z = ZWork::default();
        assert_eq!(z.coded().len(), ZBUF + 4);
        assert!(z.coded().iter().all(|&b| b == 0));
    }

    /// `fdumpz` over the test host, the chip's bytes from [`chip`]; `go` is the power switch after each chunk.
    fn runz(h: &mut Host, len: u32, mut go: impl FnMut(u32) -> bool) -> Result<Dumped, Stopped> {
        h.zlen = len as usize;
        let mut buf = std::boxed::Box::new([0u8; ZCHUNK]);
        let mut z = std::boxed::Box::new(ZWork::new());
        fdumpz_run(h, &mut buf, &mut z, 0, len, |a, d| d.iter_mut().enumerate().for_each(|(i, x)| *x = chip(a + i as u32)), |done, _| go(done))
    }

    /// BACKUP-9.6: `fdumpz` stops as `fdump` does: CAN, no sync, no ACK, the power switch, an end frame never
    /// acknowledged; an empty range sends the end frame alone.
    #[test]
    fn fdumpz_stops() {
        let len = 2 * ZCHUNK as u32;
        let mut h = Host::new();
        h.cancel_at = Some(1);
        assert_eq!(runz(&mut h, len, |_| true), Err(Stopped { at: ZCHUNK as u32, why: "cancelled by the host" }));
        let mut h = Host::new();
        h.to_radio = b"FDS2".iter().copied().collect();
        assert_eq!(runz(&mut h, len, |_| true), Err(Stopped { at: 0, why: "no sync from the host" }));
        let mut h = Host::new();
        h.silent = (0..TRIES).collect();
        assert_eq!(runz(&mut h, len, |_| true), Err(Stopped { at: 0, why: "no acknowledgement" }));
        assert_eq!(h.frames, TRIES, "sent once and resent TRIES - 1 times");
        let mut h = Host::new();
        assert_eq!(runz(&mut h, len, |_| false), Err(Stopped { at: ZCHUNK as u32, why: "the power switch is off" }));
        assert!(h.end.is_none());
        let mut h = Host::new();
        h.silent = (1..=TRIES).collect();
        assert_eq!(runz(&mut h, 100, |_| true), Err(Stopped { at: 100, why: "end frame not acknowledged" }));
        assert_eq!(h.data, (0..100).map(chip).collect::<Vec<_>>());
        let mut h = Host::new();
        let d = runz(&mut h, 0, |_| true).unwrap();
        assert_eq!((d.crc, d.blocks, d.resends), (0, 0, 0));
        assert_eq!(h.end.unwrap(), end_payload(0, 0, 0, 0));
    }

    /// BACKUP-9.5: the power switch between blocks, and an end frame never acknowledged, stop `fdump` too.
    #[test]
    fn fdump_stops_at_the_end() {
        let mut h = Host::new();
        let mut buf = [0u8; BLOCK];
        let r = fdump_run(&mut h, &mut buf, 0, 2 * BLOCK as u32, |_, d| d.fill(0), |_, _| false);
        assert_eq!(r, Err(Stopped { at: BLOCK as u32, why: "the power switch is off" }));
        let mut h = Host::new();
        h.silent = (1..=TRIES).collect();
        assert_eq!(run(&mut h, 0, 10), Err(Stopped { at: 10, why: "end frame not acknowledged" }));
        assert_eq!(h.data, (0..10).map(chip).collect::<Vec<_>>());
    }
}
