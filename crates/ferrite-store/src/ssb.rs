//! The SSB patch in SPI flash (HF-18 to HF-21): installing the flasher's bundle or a link import (`SSB_WRITE`,
//! LINK-73), and checking it. Only SSB_PATCH is written; a format leaves it alone (SAFE-9). Both installs take the
//! same steps: the region erased, the patch programmed, then [`seal`] writes the header only if the bytes check.

use crate::io::Flash;
use crate::{Nor, NorError, Result, SECTOR};
use ferrite_core::layout::SSB_PATCH;
use ferrite_core::ssb::{self, Patch, HEADER};

/// Where the patch's bytes start, after the header.
const BODY: u32 = SSB_PATCH.start + HEADER as u32;

/// The stored patch, if its header parses, its records look like a patch download and its hash matches.
pub fn check<N: Nor>(f: &Flash<N>) -> Result<Option<Patch>> {
    let mut h = [0u8; HEADER];
    f.raw_read(SSB_PATCH.start, &mut h)?;
    let Some(p) = ssb::stored(&h) else { return Ok(None) };
    Ok(body_is(f, &p)?.then_some(p))
}

/// The `p.len` bytes after the header frame as patch records and hash to `p.sha`.
fn body_is<N: Nor>(f: &Flash<N>, p: &Patch) -> Result<bool> {
    let mut sha = ssb::hasher();
    let mut buf = [0u8; 256];
    let mut off = 0u32;
    while off < p.len {
        let n = (p.len - off).min(buf.len() as u32) as usize;
        f.raw_read(BODY + off, &mut buf[..n])?;
        if off == 0 && buf[0] != 0x15 {
            return Ok(false);
        }
        if !buf[..n].chunks(8).all(|r| r[0] == 0x15 || r[0] == 0x16) {
            return Ok(false);
        }
        sha.update(&buf[..n]);
        off += n as u32;
    }
    Ok(sha.finish() == p.sha)
}

/// The region erased (blank sectors skipped): the old patch, if any, is gone from here on. 4 sectors at most.
pub async fn erase<N: Nor>(f: &Flash<N>) -> Result {
    for s in SSB_PATCH.clone().step_by(SECTOR as usize) {
        if !f.blank(s, SECTOR)? {
            f.erase(s).await?;
        }
    }
    Ok(())
}

/// Patch bytes programmed at `off` into the erased region (never the header: [`seal`] writes it).
pub async fn program<N: Nor>(f: &Flash<N>, off: u32, bytes: &[u8]) -> Result {
    if BODY as u64 + off as u64 + bytes.len() as u64 > SSB_PATCH.end as u64 {
        return Err(NorError);
    }
    f.program(BODY + off, bytes).await
}

/// The header last, only once the bytes programmed check against `p` (length, framing, hash): a cut or a bad
/// transfer leaves no header, so nothing half-written ever checks. Read back after.
pub async fn seal<N: Nor>(f: &Flash<N>, p: Patch, source: u8) -> Result {
    let mut h = [0xFFu8; HEADER];
    f.raw_read(SSB_PATCH.start, &mut h)?;
    if h != [0xFF; HEADER] || HEADER as u32 + p.len > SSB_PATCH.end - SSB_PATCH.start || !body_is(f, &p)? {
        return Err(NorError);
    }
    f.program(SSB_PATCH.start, &ssb::store_header(&p, source)).await?;
    match check(f)? {
        Some(have) if have == p => Ok(()),
        _ => Err(NorError),
    }
}

/// Writes `patch` with its header, unless the same patch is there already and checks (false). Refuses a patch
/// whose bytes don't match `p` (length, framing, hash) before erasing anything.
pub async fn install<N: Nor>(f: &Flash<N>, p: Patch, patch: &[u8], source: u8) -> Result<bool> {
    if patch.len() as u32 != p.len || !ssb::records_ok(patch) || ferrite_core::sha256::sha256(patch) != p.sha {
        return Err(NorError);
    }
    if check(f)?.is_some_and(|have| have == p) {
        return Ok(false);
    }
    erase(f).await?;
    program(f, 0, patch).await?;
    seal(f, p, source).await?;
    Ok(true)
}
