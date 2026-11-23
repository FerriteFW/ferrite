//! SAME headers (Specific Area Message Encoding; manual chapter 23, WX-4, WX-5): the decoder's text
//! `ZCZC-ORG-EEE-PSSCCC-PSSCCC+TTTT-JJJHHMM-LLLLLLLL-` parsed into fixed fields, small enough to cross the
//! task channel to the UI (which turns it into the alert screen's record). No heap; up to [`AREAS`] location
//! codes are kept, the count of all of them too.

/// Location codes kept (a header may carry 31).
pub const AREAS: usize = 4;

/// One parsed header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SameHeader {
    /// Originator: WXR, CIV, EAS, PEP.
    pub org: [u8; 3],
    /// Event code: TOR, SVR, RWT, ...
    pub event: [u8; 3],
    /// The first location codes, PSSCCC as a number (039173), and how many the header had.
    pub areas: [u32; AREAS],
    pub n_areas: u8,
    /// Valid for this long after issue, minutes (+TTTT).
    pub purge_min: u16,
    /// Issued: day of the year (1-366) and minutes since midnight, UTC (JJJHHMM).
    pub issued_day: u16,
    pub issued_min: u16,
    /// The sender (LLLLLLLL: "KCLE/NWS", "EC/GC/CA"), space padded.
    pub sender: [u8; 8],
}

/// A plain loop, no iterator adaptor: the Radio task parses headers from interrupt context, and a generic
/// `Iterator::all` the compiler chose not to inline landed in slow flash on one host's build (slowcheck, Oct 28).
fn digits(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut n = 0u32;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        // a run of digits too long for any field (a damaged header) is refused, not wrapped
        n = n.checked_mul(10)?.checked_add((c - b'0') as u32)?;
    }
    Some(n)
}

/// The header's start. A named `static`: the Radio task parses headers, and a short unnamed literal can land in
/// front of a table in the slow flash, where `slowcheck` takes it for a table read from interrupt context.
static ZCZC: [u8; 5] = *b"ZCZC-";

/// Parses a header, with or without the leading `ZCZC-` and the trailing dash. None if it isn't one.
pub fn parse(h: &[u8]) -> Option<SameHeader> {
    let h = h.strip_prefix(&ZCZC[..]).unwrap_or(h);
    let h = h.strip_suffix(b"-").unwrap_or(h);
    // ORG-EEE-areas+TTTT-JJJHHMM-LLLLLLLL
    let plus = h.iter().position(|&c| c == b'+')?;
    let (head, tail) = (&h[..plus], &h[plus + 1..]);
    let mut parts = head.split(|&c| c == b'-');
    let org = parts.next().filter(|o| o.len() == 3)?;
    let event = parts.next().filter(|e| e.len() == 3)?;
    let mut s = SameHeader {
        org: [org[0], org[1], org[2]],
        event: [event[0], event[1], event[2]],
        areas: [0; AREAS],
        n_areas: 0,
        purge_min: 0,
        issued_day: 0,
        issued_min: 0,
        sender: [b' '; 8],
    };
    for a in parts {
        let code = digits(a).filter(|_| a.len() == 6)?;
        if (s.n_areas as usize) < AREAS {
            s.areas[s.n_areas as usize] = code;
        }
        s.n_areas = s.n_areas.saturating_add(1);
    }
    if s.n_areas == 0 {
        return None;
    }
    let mut t = tail.split(|&c| c == b'-');
    let purge = t.next().filter(|p| p.len() == 4)?;
    s.purge_min = (digits(&purge[..2])? * 60 + digits(&purge[2..])?) as u16;
    let issued = t.next().filter(|i| i.len() == 7)?;
    s.issued_day = digits(&issued[..3])? as u16;
    let (hh, mm) = (digits(&issued[3..5])?, digits(&issued[5..])?);
    if hh > 23 || mm > 59 {
        return None;
    }
    s.issued_min = (hh * 60 + mm) as u16;
    let sender = t.next()?;
    let n = sender.len().min(8);
    s.sender[..n].copy_from_slice(&sender[..n]);
    Some(s)
}

/// Common event codes (NWS and Environment Canada), with their names. Codes not here show as the code.
const EVENTS: [(&[u8; 3], &str); 28] = [
    (b"TOR", "Tornado warning"),
    (b"TOA", "Tornado watch"),
    (b"SVR", "Severe thunderstorm warning"),
    (b"SVA", "Severe thunderstorm watch"),
    (b"FFW", "Flash flood warning"),
    (b"FFA", "Flash flood watch"),
    (b"FLW", "Flood warning"),
    (b"FLA", "Flood watch"),
    (b"WSW", "Winter storm warning"),
    (b"WSA", "Winter storm watch"),
    (b"BZW", "Blizzard warning"),
    (b"HWW", "High wind warning"),
    (b"HUW", "Hurricane warning"),
    (b"HUA", "Hurricane watch"),
    (b"TRW", "Tropical storm warning"),
    (b"SMW", "Special marine warning"),
    (b"EWW", "Extreme wind warning"),
    (b"TSW", "Tsunami warning"),
    (b"TSA", "Tsunami watch"),
    (b"CEM", "Civil emergency message"),
    (b"EVI", "Evacuation immediate"),
    (b"SPW", "Shelter in place warning"),
    (b"CAE", "Child abduction emergency"),
    (b"EAN", "Emergency action notification"),
    (b"SVS", "Severe weather statement"),
    (b"SPS", "Special weather statement"),
    (b"RWT", "Required weekly test"),
    (b"RMT", "Required monthly test"),
];

/// The event's name, if it is a common one.
pub fn event_name(code: &[u8; 3]) -> Option<&'static str> {
    EVENTS.iter().find(|(c, _)| *c == code).map(|&(_, n)| n)
}

/// Tests and practice: logged and toasted, never the full-screen alert.
pub fn is_test(code: &[u8; 3]) -> bool {
    matches!(code, b"RWT" | b"RMT" | b"NPT" | b"DMO")
}

/// The originator's short name.
pub fn org_name(org: &[u8; 3]) -> &'static str {
    match org {
        b"WXR" => "Weather service",
        b"CIV" => "Civil authority",
        b"EAS" => "Broadcaster",
        b"PEP" => "Primary entry",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_header() {
        let h = parse(b"ZCZC-WXR-TOR-039173-039051-139069+0030-1591829-KCLE/NWS-").unwrap();
        assert_eq!((&h.org, &h.event), (b"WXR", b"TOR"));
        assert_eq!((h.n_areas, h.areas[0], h.areas[2]), (3, 39173, 139069));
        assert_eq!(h.purge_min, 30);
        assert_eq!((h.issued_day, h.issued_min), (159, 18 * 60 + 29));
        assert_eq!(&h.sender, b"KCLE/NWS");
        assert_eq!(event_name(&h.event), Some("Tornado warning"));
        assert!(!is_test(&h.event));
        assert!(is_test(&parse(b"WXR-RWT-000000+0015-0011200-EC/GC/CA").unwrap().event));
    }

    #[test]
    fn refuses_what_isnt_one() {
        for bad in [&b"ZCZC-WXR-TOR+0030-1591829-KCLE/NWS-"[..], b"NNNN", b"ZCZC-WXR-TOR-03917+0030-1591829-X-", b"ZCZC-WXR-TOR-039173+0030-1592599-X-"] {
            assert_eq!(parse(bad), None, "{:?}", core::str::from_utf8(bad));
        }
    }

    /// Every field's own refusals: letters in a number, the wrong lengths, an hour or minute out of range, no
    /// sender; a short originator or event.
    #[test]
    fn refuses_bad_fields() {
        for bad in [
            &b"WXR-TOR-03917A+0030-1591829-X"[..],
            b"WXR-TOR-039173+00A0-1591829-X",
            b"WXR-TOR-039173+003-1591829-X",
            b"WXR-TOR-039173+0030-159182-X",
            b"WXR-TOR-039173+0030-15A1829-X",
            b"WXR-TOR-039173+0030-1592429-X",
            b"WXR-TOR-039173+0030-1591829",
            b"WX-TOR-039173+0030-1591829-X",
            b"WXR-TO-039173+0030-1591829-X",
            b"WXR-TOR--+0030-1591829-X",
        ] {
            assert_eq!(parse(bad), None, "{:?}", core::str::from_utf8(bad));
        }
    }

    /// A location code of more digits than a u32 holds is refused (it overflowed: found by fuzzing).
    #[test]
    fn refuses_a_long_run_of_digits() {
        assert_eq!(parse(b"ZCZC-CIV-CEM-0240310024033-024511-024005+0100-3651430-WBAL/TV -"), None);
        assert_eq!(parse(b"WXR-TOR-99999999999999999999+0030-1591829-X"), None);
        assert_eq!(digits(b"4294967295"), Some(u32::MAX));
        assert_eq!(digits(b"4294967296"), None);
    }

    /// WX-5: more areas than are kept still count; a sender longer than 8 is cut; no trailing dash is fine.
    #[test]
    fn many_areas_and_a_long_sender() {
        let h = parse(b"ZCZC-CIV-CEM-000001-000002-000003-000004-000005-000006+0100-0010000-LONGSENDER9").unwrap();
        assert_eq!((h.n_areas, h.areas), (6, [1, 2, 3, 4]));
        assert_eq!((h.purge_min, h.issued_day, h.issued_min), (60, 1, 0));
        assert_eq!(&h.sender, b"LONGSEND");
        assert_eq!(org_name(&h.org), "Civil authority");
        assert_eq!(event_name(&h.event), Some("Civil emergency message"));
    }

    /// WX-5's originator names; an unknown code has none, an unknown event no name.
    #[test]
    fn names() {
        let orgs = [b"WXR", b"CIV", b"EAS", b"PEP", b"XYZ"].map(org_name);
        assert_eq!(orgs, ["Weather service", "Civil authority", "Broadcaster", "Primary entry", "Unknown"]);
        assert_eq!(event_name(b"ZZZ"), None);
        assert!(EVENTS.iter().all(|(c, n)| event_name(c) == Some(*n)), "every code finds its own name");
        assert!(is_test(b"NPT") && is_test(b"DMO") && is_test(b"RMT"));
    }
}
