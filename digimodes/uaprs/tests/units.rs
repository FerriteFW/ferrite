//! AX.25, KISS, building packets, beaconing and geometry.

use uaprs::ax25::{Address, Frame, Tnc2};
use uaprs::beacon::{Beaconer, SmartBeacon};
use uaprs::build::{self, Report};
use uaprs::geo::distance_bearing;
use uaprs::kiss;
use uaprs::parse::{parse, Format, MessageKind, Packet, Timestamp};

fn a(t: &str) -> Address {
    Address::from_text(t.as_bytes()).unwrap()
}

#[test]
fn addresses() {
    assert_eq!(a("N0CALL-9").to_string(), "N0CALL-9");
    assert_eq!(a("wide2-1").to_string(), "WIDE2-1");
    assert!(a("N6EX-5*").repeated);
    for bad in ["", "TOOLONG1", "N0CALL-16", "N0_CALL", "N0CALL-", "-1", "N0CALL-123"] {
        assert!(Address::from_text(bad.as_bytes()).is_none(), "{bad}");
    }
}

#[test]
fn frame_round_trip_and_tnc2() {
    let path = [a("WIDE1-1"), a("N6EX-5*")];
    let f = Frame::new(a("N0CALL-9"), a("APRS"), &path, b"!4903.50N/07201.75W-Test\r").unwrap();
    let mut buf = [0u8; 400];
    let n = f.write(&mut buf).unwrap();
    assert_eq!(n, f.len());
    let g = Frame::parse(&buf[..n]).unwrap();
    assert_eq!(g.to_string(), "N0CALL-9>APRS,WIDE1-1,N6EX-5*:!4903.50N/07201.75W-Test<0x0d>");
    let mut text = Vec::new();
    g.emit_tnc2(|b| text.push(b));
    assert_eq!(text, g.to_string().as_bytes());
    let mut ssid = Vec::new();
    a("N0CALL-15").emit(&mut |b| ssid.push(b));
    assert_eq!(ssid, b"N0CALL-15");
    assert!(g.src.same(&a("N0CALL-9")) && g.dest.same(&a("APRS")));
    assert_eq!(g.digis().len(), 2);
    // Truncated, wrong control or PID, or nine digipeaters: rejected.
    assert!(Frame::parse(&buf[..20]).is_none());
    let mut bad = buf;
    bad[28] = 0x13;
    assert!(Frame::parse(&bad[..n]).is_none());
    assert!(Frame::new(a("N0CALL"), a("APRS"), &[a("WIDE1-1"); 9], b"").is_none());
    assert!(f.write(&mut [0u8; 20]).is_none());
}

#[test]
fn tnc2_split() {
    let t = Tnc2::split(b"N0CALL>APRS,WIDE1-1,qAR,IGATE:>status: with colons").unwrap();
    assert_eq!((t.src, t.dest, t.path, t.info), (&b"N0CALL"[..], &b"APRS"[..], &b"WIDE1-1,qAR,IGATE"[..], &b">status: with colons"[..]));
    let t = Tnc2::split(b"N0CALL>APRS:x").unwrap();
    assert!(t.path.is_empty());
    assert!(Tnc2::split(b"no header").is_none());
}

#[test]
fn kiss_round_trip() {
    let frame = [0x82, 0xC0, 0xDB, 0x00, 0xC0, 0xDB, 0xDC, 0xDD];
    let mut wire = Vec::new();
    kiss::encode(2, &frame, |b| wire.push(b));
    assert_eq!(wire.iter().filter(|&&b| b == kiss::FEND).count(), 2);
    let mut d: kiss::Decoder = kiss::Decoder::new();
    let mut got = Vec::new();
    // Noise before the first FEND and back-to-back FENDs are ignored.
    for b in [0x55, 0x66, kiss::FEND].iter().chain(&wire) {
        if let Some(r) = d.push(*b) {
            got.push((r.port, r.command, r.data.to_vec()));
        }
    }
    assert_eq!(got, vec![(2, 0, frame.to_vec())]);
    // A frame longer than the buffer is dropped, the next one is not.
    let mut small: kiss::Decoder<4> = kiss::Decoder::new();
    let mut n = 0;
    for b in [kiss::FEND, 0, 1, 2, 3, 4, 5, kiss::FEND, 0, 9, kiss::FEND] {
        if let Some(r) = small.push(b) {
            assert_eq!(r.data, [9]);
            n += 1;
        }
    }
    assert_eq!(n, 1);
}

fn reparse(info: &[u8]) -> Packet<'_> {
    parse(b"APRS", info).unwrap()
}

#[test]
fn build_positions_parse_back() {
    let places = [(49_058_333, -72_029_167), (-33_850_000, 151_200_000), (0, 0), (89_999_000, 179_999_000), (-45_000_001, -1)];
    for (lat, lon) in places {
        for ts in [None, Some(Timestamp::DayHourMin { day: 9, hour: 23, min: 45, utc: true })] {
            let r = Report {
                lat,
                lon,
                table: b'/',
                code: b'>',
                course_speed: Some((88, 36)),
                altitude: Some(1234),
                messaging: true,
                timestamp: ts,
                comment: b"Ferrite test",
                ambiguity: 0,
            };
            let mut buf = [0u8; 128];
            let n = build::position(&r, &mut buf).unwrap();
            let Packet::Position { pos, messaging, timestamp, comment, .. } = reparse(&buf[..n]) else { panic!() };
            assert!((pos.lat - lat).abs() <= 85 && (pos.lon - lon).abs() <= 85, "{lat} {lon}: {pos:?}");
            assert_eq!((pos.course, pos.speed, pos.altitude), (Some(88), Some(36), Some(1234)));
            assert!(messaging);
            assert_eq!(timestamp, ts);
            assert_eq!(comment.bytes().collect::<Vec<_>>(), b"Ferrite test");

            let n = build::compressed(&r, &mut buf).unwrap();
            let Packet::Position { pos, comment, .. } = reparse(&buf[..n]) else { panic!() };
            assert_eq!(pos.format, Format::Compressed);
            assert!((pos.lat - lat).abs() <= 3 && (pos.lon - lon).abs() <= 6, "{lat} {lon}: {pos:?}");
            assert_eq!(pos.course, Some(88));
            assert!((pos.speed.unwrap() as i32 - 36).abs() <= 1);
            assert_eq!(pos.altitude, Some(1234));
            assert_eq!(comment.bytes().collect::<Vec<_>>(), b"Ferrite test");
        }
    }
}

/// APRS-50: position ambiguity blanks the same trailing digits of latitude and longitude, and a receiver reads
/// it back as such (the middle of the box).
#[test]
fn build_position_ambiguity() {
    // 49 03.50 N, 072 01.75 W
    let want = [
        "=4903.50N/07201.75W[",
        "=4903.5 N/07201.7 W[",
        "=4903.  N/07201.  W[",
        "=490 .  N/0720 .  W[",
        "=49  .  N/072  .  W[",
    ];
    let mut buf = [0u8; 64];
    for (amb, w) in want.iter().enumerate() {
        let r = Report { lat: 49_058_333, lon: -72_029_167, table: b'/', code: b'[', course_speed: None, altitude: None, messaging: true, timestamp: None, comment: b"", ambiguity: amb as u8 };
        let n = build::position(&r, &mut buf).unwrap();
        assert_eq!(core::str::from_utf8(&buf[..n]).unwrap(), *w);
        let Packet::Position { pos, .. } = reparse(&buf[..n]) else { panic!() };
        assert_eq!(pos.ambiguity, amb as u8);
        // within half the box the blanks leave (0.01, 0.1, 1, 10 minutes, then a degree) and the rounding
        let box_ = [85, 920, 8_420, 83_420, 500_085][amb];
        assert!((pos.lat - 49_058_333).abs() <= box_ && (pos.lon + 72_029_167).abs() <= box_, "{amb}: {pos:?}");
    }
    // more than 4 reads as 4
    let r = Report { lat: 49_058_333, lon: -72_029_167, table: b'/', code: b'[', course_speed: None, altitude: None, messaging: true, timestamp: None, comment: b"", ambiguity: 9 };
    let n = build::position(&r, &mut buf).unwrap();
    assert_eq!(&buf[..n], want[4].as_bytes());
}

#[test]
fn build_messages_parse_back() {
    let mut buf = [0u8; 100];
    let n = build::message(b"N0CALL-9", b"Hello from Ferrite", Some(b"42"), &mut buf).unwrap();
    assert_eq!(&buf[..n], b":N0CALL-9 :Hello from Ferrite{42");
    let Packet::Message(m) = reparse(&buf[..n]) else { panic!() };
    assert_eq!((m.addressee, m.text, m.id, m.kind), (&b"N0CALL-9"[..], &b"Hello from Ferrite"[..], Some(&b"42"[..]), MessageKind::Message));
    let n = build::ack(b"N0CALL-9", b"42", &mut buf).unwrap();
    let Packet::Message(m) = reparse(&buf[..n]) else { panic!() };
    assert_eq!((m.kind, m.id), (MessageKind::Ack, Some(&b"42"[..])));
    let n = build::rej(b"N0CALL-9", b"42", &mut buf).unwrap();
    let Packet::Message(m) = reparse(&buf[..n]) else { panic!() };
    assert_eq!(m.kind, MessageKind::Rej);
    // Out of range: addressee over 9, id over 5, text over 67 or with a '{'.
    assert!(build::message(b"TOOLONGNAME", b"x", None, &mut buf).is_none());
    assert!(build::message(b"N0CALL", b"x", Some(b"123456"), &mut buf).is_none());
    assert!(build::message(b"N0CALL", &[b'x'; 68], None, &mut buf).is_none());
    assert!(build::message(b"N0CALL", b"a{b", None, &mut buf).is_none());
    assert!(build::message(b"N0CALL", b"x", None, &mut [0u8; 5]).is_none());
}

#[test]
fn smart_beacon() {
    let mut b = Beaconer::new(SmartBeacon::default());
    assert!(b.due(0, 0, 0)); // nothing sent yet
    b.sent(0, 90);
    // Parked: every 30 minutes.
    assert!(!b.due(1799, 0, 90));
    assert!(b.due(1800, 0, 90));
    // 10 km/h and over: every 3 minutes; half the fast speed (a walk): twice that; a slow amble capped at parked.
    assert!(b.due(180, 120, 90) && !b.due(179, 120, 90));
    assert!(b.due(180, 50, 90) && !b.due(179, 50, 90));
    assert!(b.due(360, 5, 90) && !b.due(359, 5, 90));
    assert!(b.due(600, 3, 90) && !b.due(599, 3, 90));
    // A turn: 28 + 255 / 50 = 33 degrees at 50 km/h, after 30 s.
    assert!(b.due(31, 50, 90 + 34) && !b.due(31, 50, 90 + 32) && !b.due(29, 50, 90 + 90));
    // Across north.
    b.sent(1000, 350);
    assert!(b.due(1040, 50, 25) && !b.due(1040, 50, 10));
}

#[test]
fn distances_and_bearings() {
    // Reference values from the haversine formula on a 6371 km sphere.
    let cases = [
        ((49_058_333, -72_029_167), (49_158_333, -72_029_167), 11_119, 0),
        ((49_000_000, -72_000_000), (49_000_000, -71_900_000), 7_295, 90),
        ((33_750_000, -118_000_000), (34_052_000, -118_244_000), 40_433, 326),
        ((0, 179_950_000), (0, -179_950_000), 11_119, 90),
        ((-33_850_000, 151_200_000), (-34_850_000, 150_200_000), 144_193, 219),
    ];
    for (from, to, metres, bearing) in cases {
        let (d, b) = distance_bearing(from, to);
        assert!((d as i64 - metres).abs() * 200 <= metres, "{from:?} -> {to:?}: {d} m, want {metres}");
        assert!(((b as i32 - bearing + 540) % 360 - 180).abs() <= 1, "{from:?} -> {to:?}: {b} degrees, want {bearing}");
    }
    assert_eq!(distance_bearing((1, 1), (1, 1)), (0, 0));
}

#[test]
fn hostile_packets_do_not_panic() {
    // Every vector packet, mutated: bytes replaced, inserted, deleted, truncated; and random bytes. Runs with
    // overflow checks in debug builds, so arithmetic overflow fails here too.
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    for set in ["wa8lmf", "suite", "spec"] {
        let path = format!("{}/../../../testdata/aprs/{set}.hex", env!("CARGO_MANIFEST_DIR"));
        if let Ok(t) = std::fs::read_to_string(path) {
            seeds.extend(t.lines().map(|l| (0..l.len() / 2).map(|i| u8::from_str_radix(&l[2 * i..2 * i + 2], 16).unwrap()).collect()));
        }
    }
    for l in include_str!("spec_packets.txt").lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        seeds.push(l.as_bytes().to_vec());
    }
    let mut s = 0x1234_5678_9ABC_DEF0u64;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let specials = [0u8, b' ', b'/', b'-', b'!', b'}', b'{', b'.', b'9', b'0', 0x7F, 0xFF, b':', b'*'];
    for round in 0..60_000usize {
        let mut p = seeds[round % seeds.len()].clone();
        for _ in 0..1 + rnd() % 4 {
            let n = p.len().max(1);
            let at = (rnd() as usize) % n;
            let b = if rnd() % 2 == 0 { specials[rnd() as usize % specials.len()] } else { rnd() as u8 };
            match rnd() % 4 {
                0 if !p.is_empty() => p[at] = b,
                1 => p.insert(at.min(p.len()), b),
                2 if !p.is_empty() => {
                    p.remove(at);
                }
                _ => p.truncate(at),
            }
        }
        if round % 10 == 0 {
            p = (0..rnd() % 80).map(|_| rnd() as u8).collect();
        }
        if let Some(t) = Tnc2::split(&p) {
            let mut got = parse(t.dest, t.info);
            while let Ok(Packet::ThirdParty(inner)) = got {
                got = parse(inner.dest, inner.info);
            }
        }
        if let Some(f) = Frame::parse(&p) {
            let _ = parse(f.dest.call(), f.info);
            let _ = f.to_string();
        }
        let mut d: kiss::Decoder<64> = kiss::Decoder::new();
        for &b in &p {
            let _ = d.push(b);
        }
    }
}

#[test]
fn edge_values() {
    // Mic-E altitude at its maximum, Ultimeter extremes, overlong NMEA numbers, NMEA out of range.
    assert!(matches!(parse(b"S32U6T", b"`d(((((>/{{{}"), Ok(Packet::Position { .. }) | Err(_)));
    assert!(parse(b"APRS", b"$ULTW7FFF80007FFF80007FFF80007FFF80007FFF80007FFF80007FFF").is_ok());
    // An absurd speed loses the speed, not the position (raw NMEA is a feature: the radio builds without it).
    #[cfg(feature = "nmea")]
    {
        let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"$GPRMC,013244,A,4903.50,N,07201.75,W,4294967296,088.0,240512,,") else { panic!() };
        assert_eq!(pos.speed, None);
        let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,99999999999.5,M,46.9,M,,") else { panic!() };
        assert_eq!(pos.altitude, None);
        assert!(parse(b"APRS", b"$GPGLL,9903.50,N,99901.75,W,225444,A").is_err());
    }
    // Weather with a sign on a field that cannot be negative.
    let Ok(Packet::Weather { weather, .. }) = parse(b"APRS", b"_10090556c-12s004t-05") else { panic!() };
    assert_eq!(weather.wind_dir, None);
    // Impossible timestamps.
    assert!(parse(b"APRS", b"/992345z4903.50N/07201.75W>").is_err());
    assert!(parse(b"APRS", b"@092375z4903.50N/07201.75W>").is_err());
}

#[test]
fn dao_keeps_the_hemisphere_at_zero() {
    let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"!0000.00S/00000.00W>!W99!") else { panic!() };
    assert_eq!((pos.lat, pos.lon), (-150, -150));
    let Ok(Packet::Position { pos, .. }) = parse(b"APRS", b"!4903.50S/07201.75W>!W12!") else { panic!() };
    assert_eq!((pos.lat, pos.lon), (-49_058_350, -72_029_200));
}

#[test]
fn builder_courses_near_north_and_data_like_comments() {
    let mut buf = [0u8; 128];
    for course in [1u16, 2, 3, 357, 358, 359, 360] {
        let r = Report { lat: 1, lon: 1, table: b'/', code: b'>', course_speed: Some((course, 20)), altitude: None, messaging: false, timestamp: None, comment: b"", ambiguity: 0 };
        let n = build::compressed(&r, &mut buf).unwrap();
        let Packet::Position { pos, .. } = reparse(&buf[..n]) else { panic!() };
        // Within 2 degrees of north, compressed course reads back as "none" (c = 0); never as a range.
        assert_eq!(pos.range, None, "course {course}");
        assert!(pos.speed.is_some());
        if let Some(c) = pos.course {
            assert!(((c as i32 - course as i32 + 540) % 360 - 180).abs() <= 2, "course {course} -> {c}");
        }
    }
    for comment in [&b"123/045 hello"[..], b"RNG0050 ok", b"PHG5132 x", b"/slash first", b" space first"] {
        for (cs, alt) in [(None, None), (Some((90u16, 10u16)), None), (None, Some(100))] {
            let r = Report { lat: 1, lon: 1, table: b'/', code: b'>', course_speed: cs, altitude: alt, messaging: false, timestamp: None, comment, ambiguity: 0 };
            let n = build::position(&r, &mut buf).unwrap();
            let Packet::Position { pos, comment: c, .. } = reparse(&buf[..n]) else { panic!() };
            assert_eq!(c.bytes().collect::<Vec<_>>(), comment.trim_ascii(), "{}", String::from_utf8_lossy(&buf[..n]));
            assert_eq!((pos.course, pos.speed, pos.range, pos.phg), (cs.map(|c| c.0), cs.map(|c| c.1), None, None));
        }
    }
}

/// KISS-11: framing as the KISS spec: a bad byte after FESC is dropped, a frame cut off mid-escape too.
#[test]
fn kiss_escapes_and_full_buffer() {
    let mut d: kiss::Decoder<8> = kiss::Decoder::new();
    let mut got = Vec::new();
    // A bad escape drops its byte (the spec: "no action is taken"); FESC then FEND ends the frame mid-escape, so
    // it is dropped.
    for b in [kiss::FEND, 0, 1, kiss::FESC, 0x42, 2, kiss::FEND, 0, kiss::FESC, kiss::FEND, 0, 7, kiss::FEND] {
        if let Some(r) = d.push(b) {
            got.push(r.data.to_vec());
        }
    }
    assert_eq!(got, vec![vec![1, 2], vec![7]]);
    // Exactly full is fine.
    let mut d: kiss::Decoder<4> = kiss::Decoder::new();
    let mut n = 0;
    for b in [kiss::FEND, 0, 1, 2, 3, kiss::FEND] {
        n += d.push(b).is_some_and(|r| r.data == [1, 2, 3]) as u32;
    }
    assert_eq!(n, 1);
}

#[test]
fn smart_beacon_odd_settings() {
    let cfg = SmartBeacon { slow_speed: 0, fast_speed: 65535, fast_rate: u32::MAX, ..SmartBeacon::default() };
    let mut b = Beaconer::new(cfg);
    b.sent(0, 0);
    let _ = b.due(10, 0, 0);
    let _ = b.due(10, 1, 180);
}

#[test]
fn geo_far_and_polar() {
    let check = |from: (i32, i32), to: (i32, i32), metres: i64| {
        let (d, _) = distance_bearing(from, to);
        assert!((d as i64 - metres).abs() * 200 <= metres, "{from:?} -> {to:?}: {d} m, want {metres}");
    };
    // 1000 km east-west at 45 N, and north-south near the pole (haversine references).
    check((45_000_000, 0), (45_000_000, 12_700_000), 997_535);
    check((80_000_000, 10_000_000), (88_000_000, 10_000_000), 889_559);
}

/// KISS-11: commands 1-6 and Return, p-persistence, full duplex never skipping the channel check, port 0 only.
#[test]
fn kiss_settings_and_channel_access() {
    use kiss::{Access, Command, Params, Received};
    let mut p = Params::default();
    let r = |port: u8, command: u8, data: &'static [u8]| Received { port, command, data };
    assert_eq!(p.apply(&r(0, 1, &[30])), Command::Set);
    assert_eq!(p.apply(&r(0, 2, &[127])), Command::Set);
    assert_eq!(p.apply(&r(0, 3, &[5])), Command::Set);
    assert_eq!(p.apply(&r(0, 4, &[2])), Command::Set);
    assert_eq!((p.tx_delay, p.persistence, p.slot_time, p.tx_tail), (30, 127, 5, 2));
    assert_eq!(p.apply(&r(0, 0, b"frame")), Command::Data(b"frame"));
    assert_eq!(p.apply(&r(0, 6, &[1, 2])), Command::Hardware(&[1, 2]));
    assert_eq!(p.apply(&r(15, 15, &[])), Command::Exit);
    assert_eq!(p.apply(&r(0, 1, &[])), Command::Unknown);
    // 300 ms at 1200 baud is 45 flags; no tail still gets one closing flag.
    assert_eq!(Params { tx_delay: 30, tx_tail: 0, ..p }.flags(1200), (45, 1));

    let mut a = Access::default();
    // Busy: never. Clear and lucky: now. Clear and unlucky: not until a slot time has passed.
    assert!(!a.may_send(&p, 0, true, 0));
    assert!(a.may_send(&p, 10, false, 127));
    assert!(!a.may_send(&p, 20, false, 128));
    assert!(!a.may_send(&p, 60, false, 0)); // slot is 50 ms
    assert!(a.may_send(&p, 70, false, 0));
    // The channel going busy cancels the wait; full duplex from the host changes nothing (half-duplex radio).
    assert!(!a.may_send(&p, 80, false, 200));
    assert!(!a.may_send(&p, 90, true, 0));
    assert!(a.may_send(&p, 95, false, 0));
    assert!(!a.may_send(&Params { full_duplex: true, ..p }, 100, true, 0), "busy stays busy");
    // A single-port TNC takes port 0 and Return only.
    assert!(r(0, 0, b"x").for_single_port() && r(15, 15, &[]).for_single_port());
    assert!(!r(1, 0, b"x").for_single_port() && !r(2, 1, &[30]).for_single_port());
    // Over many tries, P = 63 sends about a quarter of the time in free slots.
    let p = Params::default();
    let mut a = Access::default();
    let (mut sent, mut tries, mut t, mut x) = (0, 0, 0u32, 1u32);
    while tries < 4000 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        if a.may_send(&p, t, false, (x >> 16) as u8) {
            sent += 1;
        }
        tries += 1;
        t += 100;
    }
    assert!((800..1200).contains(&sent), "{sent} of {tries}");
}

/// KISS-12: hardware commands (command 6) never become data or settings: Mobilinkd's PTT test tones (0x07-0x09),
/// a host's frequency set (0xEA) and save (0x2A) come back as `Hardware`, which the firmware ignores, and leave
/// the TNC settings as they were.
#[test]
fn kiss_hardware_commands_do_nothing() {
    use kiss::{Command, Params, Received};
    let mut p = Params::default();
    let before = p;
    for sub in [&[0x07u8][..], &[0x08], &[0x09], &[0xEA, 0x08, 0xAA, 0x6B, 0x60], &[0x2A], &[0x28]] {
        let r = Received { port: 0, command: 6, data: sub };
        assert_eq!(p.apply(&r), Command::Hardware(sub));
    }
    assert_eq!(p, before);
}

fn position_of(dest: &[u8], info: &[u8]) -> uaprs::parse::Position {
    match parse(dest, info) {
        Ok(Packet::Position { pos, .. }) => pos,
        other => panic!("{:?}: {other:?}", core::str::from_utf8(info)),
    }
}

fn comment_of(info: &[u8]) -> Vec<u8> {
    match parse(b"APRS", info) {
        Ok(Packet::Position { comment, .. }) => comment.bytes().collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn comment_length_after_cuts() {
    // The altitude is cut out of the comment; its length counts what is left.
    let Ok(Packet::Position { pos, comment, .. }) = parse(b"APRS", b"!4903.50N/07201.75W>/A=001234 hello") else { panic!() };
    assert_eq!(pos.altitude, Some(1234));
    assert_eq!((comment.len(), comment.is_empty()), (5, false));
    assert_eq!(comment.raw(), b"hello");
    let Ok(Packet::Position { comment, .. }) = parse(b"APRS", b"!4903.50N/07201.75W>") else { panic!() };
    assert_eq!((comment.len(), comment.is_empty()), (0, true));
}

#[test]
fn compressed_position_out_of_range() {
    use uaprs::parse::Error;
    // y and x at their base-91 maximum: 90.02 S, 180.03 E.
    assert_eq!(parse(b"APRS", b"!/{{{{!!!!>   "), Err(Error::Position));
    assert_eq!(parse(b"APRS", b"!/!!!!{{{{>   "), Err(Error::Position));
    // The corners themselves are fine.
    let p = position_of(b"APRS", b"!/!!!!!!!!>   ");
    assert_eq!((p.lat, p.lon), (90_000_000, -180_000_000));
}

#[test]
fn data_extensions_unknown_course_and_phgr() {
    // Course and speed sent as dots: unknown, and not left in the comment.
    let Ok(Packet::Position { pos, comment, .. }) = parse(b"APRS", b"!4903.50N/07201.75W>.../...moving") else { panic!() };
    assert_eq!((pos.course, pos.speed), (None, None));
    assert_eq!(comment.bytes().collect::<Vec<_>>(), b"moving");
    // PHGR (APRS 1.2): PHG with a beacon-rate character, then '/'.
    let Ok(Packet::Position { pos, comment, .. }) = parse(b"APRS", b"!4903.50N/07201.75W#PHG5132A/digi") else { panic!() };
    assert_eq!(pos.phg, Some(*b"5132"));
    assert_eq!(comment.bytes().collect::<Vec<_>>(), b"digi");
}

#[test]
fn dao_base91_and_datum_only() {
    // Base 91 (`!w"#!`): (x - 33) / 91 hundredths of a minute: 1/91 and 2/91, so 2 and 4 millionths of a degree,
    // north and east added.
    let p = position_of(b"APRS", b"!4903.50N/07201.75E>!w\"#!");
    assert_eq!((p.lat, p.lon), (49_058_333 + 2, 72_029_167 + 4));
    assert!(comment_of(b"!4903.50N/07201.75E>!w\"#!").is_empty());
    // A datum alone (`!W  !`): the position as sent, the extension cut from the comment.
    let p = position_of(b"APRS", b"!4903.50N/07201.75E>Comment !W  !");
    assert_eq!((p.lat, p.lon), (49_058_333, 72_029_167));
    assert_eq!(comment_of(b"!4903.50N/07201.75E>Comment !W  !"), b"Comment");
}

#[test]
fn weather_luminosity_and_unknown_fields() {
    use uaprs::parse::Weather;
    // L is luminosity under 1000 W/m², l 1000 and up; `#` (raw rain counter) is skipped; a `v` without digits
    // ends the fields.
    let Ok(Packet::Weather { timestamp, weather, comment }) = parse(b"APRS", b"_10090556c220s004g005t077L123l045#010h50b10132vXYZ") else { panic!() };
    assert_eq!(timestamp, Some(Timestamp::MonthDayHourMin { month: 10, day: 9, hour: 5, min: 56 }));
    let want = Weather {
        wind_dir: Some(220),
        wind_speed: Some(4),
        wind_gust: Some(5),
        temp: Some(77),
        humidity: Some(50),
        pressure: Some(10132),
        luminosity: Some(1045),
        ..Weather::default()
    };
    assert_eq!((weather, comment), (want, &b"vXYZ"[..]));
    let Ok(Packet::Weather { weather, .. }) = parse(b"APRS", b"_10090556c220L999") else { panic!() };
    assert_eq!(weather.luminosity, Some(999));
    // A weather station's position with its wind unknown (dots).
    let Ok(Packet::Position { weather: Some(w), .. }) = parse(b"APRS", b"!4903.50N/07201.75W_.../...g005t077") else { panic!() };
    assert_eq!((w.wind_dir, w.wind_speed, w.wind_gust, w.temp), (None, None, Some(5), Some(77)));
}

#[test]
fn mic_e_custom_messages_and_bad_fields() {
    use uaprs::parse::{Error, MicEStatus};
    let info = b"`(_fn\"Oj/";
    assert!(parse(b"S32UVT", info).is_ok());
    // Custom message bits (A-K in the first three characters); K is a custom 1 that also blanks its digit.
    let Ok(Packet::Position { mic_e, .. }) = parse(b"C32UVT", info) else { panic!() };
    assert_eq!(mic_e, Some(MicEStatus { bits: 0b100, custom: true, device: None }));
    let Ok(Packet::Position { pos, mic_e, .. }) = parse(b"A2KLLL", info) else { panic!() };
    assert_eq!(mic_e, Some(MicEStatus { bits: 0b101, custom: true, device: None }));
    assert_eq!((pos.ambiguity, pos.lat), (4, -2_500_000), "2 30 S, to the degree");
    for (dest, info, err) in [
        (&b"S32UVT"[..], &b"`(_fn\"O"[..], Error::Short),    // information field too short
        (b"S32U-T", info, Error::MicE),                      // not a Mic-E character
        (b"S3LUVT", info, Error::MicE),                      // a digit after a blank
        (b"S32CVT", info, Error::MicE),                      // A-K past the message bits
        (b"SA2UVT", info, Error::MicE),                      // standard and custom bits mixed
        (b"S32UVT", b"`(_\x80n\"Oj/", Error::MicE),          // longitude hundredths 100
        (b"S32UVT", b"`(_f\x10\"Oj/", Error::MicE),          // speed byte below 28
        (b"S32UVT", b"`(_fn\"Oj?", Error::Symbol),           // no such symbol table
    ] {
        assert_eq!(parse(dest, info), Err(err), "{:?} {:?}", core::str::from_utf8(dest), info);
    }
    // A `}` at the altitude's place without base-91 characters before it: no altitude, all comment.
    let Ok(Packet::Position { pos, comment, .. }) = parse(b"S32UVT", b"`(_fn\"Oj/||}}hi") else { panic!() };
    assert_eq!(pos.altitude, None);
    assert_eq!(comment.bytes().collect::<Vec<_>>(), b"||}}hi");
}

#[test]
fn message_needs_an_addressee() {
    assert_eq!(parse(b"APRS", b":         :hello"), Err(uaprs::parse::Error::Message));
}

#[cfg(feature = "nmea")]
#[test]
fn nmea_fix_status_rounding_and_altitude() {
    use uaprs::parse::Error;
    assert_eq!(parse(b"APRS", b"$GPGLL,4916.45,N,12311.12,W,225444,V"), Err(Error::Nmea), "GLL without a fix");
    // Speed rounds on the next digit; course 0 is north.
    let p = position_of(b"APRS", b"$GPRMC,013244,A,4903.50,N,07201.75,W,036.7,000.0,240512,,");
    assert_eq!((p.speed, p.course), (Some(37), Some(360)));
    let p = position_of(b"APRS", b"$GPRMC,013244,A,4903.50,N,07201.75,W,036.x,088.4,240512,,");
    assert_eq!((p.speed, p.course), (None, Some(88)), "a speed that is not a number is dropped");
    // Below sea level: -12.5 m is -41 feet (to the foot).
    let p = position_of(b"APRS", b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,-12.5,M,46.9,M,,");
    assert!((p.altitude.unwrap() + 41).abs() <= 1, "{:?}", p.altitude);
    // Nine digits fit, but not in tenths of a metre: no altitude.
    let p = position_of(b"APRS", b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,999999999.5,M,46.9,M,,");
    assert_eq!(p.altitude, None);
}

#[cfg(feature = "nmea")]
#[test]
fn nmea_symbol_from_the_destination() {
    // APRS 1.0.1 appendix 2: GPS, SPC or SYM, then a two-character symbol and an optional overlay; or GPSCnn
    // (primary table) and GPSEnn (alternate) by number. Anything else is the primary table's dot.
    let gga = b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,";
    for (dest, want) in [
        (&b"APRS"[..], (b'/', b'/')),
        (b"GP", (b'/', b'/')),
        (b"GPSC43", (b'/', b'K')),
        (b"GPSE01", (b'\\', b'!')),
        (b"GPSC00", (b'/', b'/')),
        (b"GPSC", (b'/', b'/')),
        (b"GPSBB", (b'/', b'!')),
        (b"SPCOBA", (b'A', b'!')),
        (b"SYMOB", (b'\\', b'!')),
        (b"GPSPA", (b'/', b'A')),
        (b"GPSA5", (b'\\', b'5')),
        (b"GPSMR", (b'/', b':')),
        (b"GPSNX", (b'\\', b'@')),
        (b"GPSHS", (b'/', b'[')),
        (b"GPSDX", (b'\\', b'`')),
        (b"GPSLA", (b'/', b'a')),
        (b"GPSSZ", (b'\\', b'z')),
        (b"GPSJ1", (b'/', b'{')),
        (b"GPSQ4", (b'\\', b'~')),
        (b"GPSZZ", (b'/', b'/')),
    ] {
        let p = position_of(dest, gga);
        assert_eq!((p.table, p.code), want, "{:?}", core::str::from_utf8(dest));
    }
}

/// Every builder writes nothing past the buffer: each buffer shorter than the packet gives None.
#[test]
fn builders_refuse_short_buffers() {
    let r = Report {
        lat: -33_850_000,
        lon: 151_200_000,
        table: b'/',
        code: b'>',
        course_speed: Some((88, 36)),
        altitude: Some(-100),
        messaging: false,
        timestamp: Some(Timestamp::HourMinSec { hour: 12, min: 34, sec: 56 }),
        comment: b"comment",
        ambiguity: 0,
    };
    let local = Report { timestamp: Some(Timestamp::DayHourMin { day: 9, hour: 23, min: 45, utc: false }), messaging: true, ..r };
    let quiet = Report { course_speed: None, altitude: None, timestamp: None, comment: b"/looks like data", ..r };
    let mic = Report { altitude: Some(200), ..r };
    let mic_quiet = Report { altitude: None, comment: b"abc}", ..r };
    type Build = Box<dyn Fn(&mut [u8]) -> Option<usize>>;
    let cases: Vec<(&str, Build)> = vec![
        ("position", Box::new(move |o| build::position(&r, o))),
        ("position local", Box::new(move |o| build::position(&local, o))),
        ("position quiet", Box::new(move |o| build::position(&quiet, o))),
        ("compressed", Box::new(move |o| build::compressed(&r, o))),
        ("compressed quiet", Box::new(move |o| build::compressed(&quiet, o))),
        ("message", Box::new(|o| build::message(b"N0CALL", b"hello", Some(b"12"), o))),
        ("ack", Box::new(|o| build::ack(b"N0CALL", b"12", o))),
        ("rej", Box::new(|o| build::rej(b"N0CALL", b"12", o))),
        ("mic_e", Box::new(move |o| build::mic_e(&mic, build::MIC_E_EN_ROUTE, &mut [0; 6], o))),
        ("mic_e quiet", Box::new(move |o| build::mic_e(&mic_quiet, build::MIC_E_EN_ROUTE, &mut [0; 6], o))),
    ];
    for (name, f) in &cases {
        let mut buf = [0u8; 128];
        let n = f(&mut buf).unwrap_or_else(|| panic!("{name}"));
        for short in 0..n {
            let mut small = vec![0u8; short];
            assert_eq!(f(&mut small), None, "{name} in {short} bytes");
        }
    }
}

#[test]
fn builder_timestamps_rounding_and_limits() {
    let base = Report {
        lat: 49_999_999,
        lon: -72_029_167,
        table: b'/',
        code: b'>',
        course_speed: None,
        altitude: Some(-100),
        messaging: false,
        timestamp: Some(Timestamp::HourMinSec { hour: 12, min: 34, sec: 56 }),
        comment: b"",
        ambiguity: 0,
    };
    let mut buf = [0u8; 128];
    // 49 59.99994 rounds up to 50 00.00; HHMMSSh; a negative altitude as -nnnnn.
    let n = build::position(&base, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"/123456h5000.00N/07201.75W>/A=-00100");
    let Packet::Position { timestamp, pos, .. } = reparse(&buf[..n]) else { panic!() };
    assert_eq!((timestamp, pos.altitude), (base.timestamp, Some(-100)));
    // Local day/hour/minute ends in '/'.
    let local = Report { timestamp: Some(Timestamp::DayHourMin { day: 9, hour: 23, min: 45, utc: false }), ..base };
    let n = build::position(&local, &mut buf).unwrap();
    assert!(buf[..n].starts_with(b"/092345/"));
    let Packet::Position { timestamp, .. } = reparse(&buf[..n]) else { panic!() };
    assert_eq!(timestamp, local.timestamp);
    // What can't be sent: a weather-only timestamp, a position off the globe, an altitude past six digits.
    let mdhm = Report { timestamp: Some(Timestamp::MonthDayHourMin { month: 1, day: 2, hour: 3, min: 4 }), ..base };
    assert_eq!(build::position(&mdhm, &mut buf), None);
    assert_eq!(build::position(&Report { lat: 90_000_001, ..base }, &mut buf), None);
    assert_eq!(build::compressed(&Report { lon: -180_000_001, ..base }, &mut buf), None);
    assert_eq!(build::position(&Report { altitude: Some(1_000_000), ..base }, &mut buf), None);
    assert_eq!(build::mic_e(&base, 8, &mut [0; 6], &mut buf), None, "no such status");
    assert_eq!(build::mic_e(&Report { lon: 180_000_001, ..base }, 0, &mut [0; 6], &mut buf), None);
    // Messages: empty or long addressees and ids, and the characters message text may not carry.
    assert_eq!(build::ack(b"", b"1", &mut buf), None);
    assert_eq!(build::rej(b"N0CALL", b"123456", &mut buf), None);
    assert_eq!(build::message(b"N0CALL", b"a{b", None, &mut buf), None);
    assert_eq!(build::message(b"N0CALL", b"hi", Some(b""), &mut buf), None);
}

#[test]
fn address_and_frame_rejects() {
    // A character outside A-Z, 0-9 in a callsign of the right length.
    assert!(Address::from_text(b"N0_CAL").is_none());
    // Fields: too short, all spaces.
    assert!(Address::from_field(&[0x9C, 0x60, 0x86]).is_none());
    assert!(Address::from_field(&[0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x61]).is_none());
    assert_eq!(format!("{:?}", a("N6EX-5*")), "N6EX-5*");
    assert_eq!(format!("{:?}", a("N6EX")), "N6EX");

    let path: Vec<Address> = (1..=8).map(|i| a(&format!("WIDE{i}"))).collect();
    let f = Frame::new(a("N0CALL"), a("APRS"), &path, b">hi").unwrap();
    assert!(!f.is_empty());
    let mut buf = [0u8; 200];
    let n = f.write(&mut buf).unwrap();
    assert!(Frame::parse(&buf[..n]).is_some());
    let bad = |edit: &dyn Fn(&mut Vec<u8>)| {
        let mut b = buf[..n].to_vec();
        edit(&mut b);
        Frame::parse(&b).is_none()
    };
    assert!(bad(&|b| b[6] |= 1), "the destination marked last");
    assert!(bad(&|b| b[7] |= 1), "a source with the extension bit inside its callsign");
    assert!(bad(&|b| b.truncate(14 + 7 * 3 + 3)), "cut inside a digipeater");
    assert!(bad(&|b| b[14 + 7 * 8 + 1] = 0xCF), "not PID 0xF0");
    // A ninth digipeater: the eighth not marked last, then another address.
    assert!(bad(&|b| {
        b[14 + 7 * 8 - 1] &= !1;
        let extra = a("WIDE9").to_field(true);
        b.splice(14 + 7 * 8..14 + 7 * 8, extra);
    }));
}

#[test]
fn display_passes_formatter_errors_on() {
    use std::fmt::{self, Write};
    /// Takes `room` bytes, then fails.
    struct Full {
        room: usize,
    }
    impl Write for Full {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.room = self.room.checked_sub(s.len()).ok_or(fmt::Error)?;
            Ok(())
        }
    }
    let f = Frame::new(a("N0CALL-9"), a("APRS"), &[a("WIDE1-1*")], b"hi\x01").unwrap();
    let full = f.to_string().len();
    for room in 0..full {
        assert!(write!(Full { room }, "{f}").is_err(), "room {room}");
    }
    assert!(write!(Full { room: full }, "{f}").is_ok());
}

/// Command 12 is ACKMODE: a byte turns it on and off, no byte is ignored, and a report round-trips with
/// command nibble 0x0C. A window's flags: the first frame carries the TX delay, a later one none, and only
/// the last frame keeps the three-flag tail (or a longer TX tail the host asked for).
#[test]
fn kiss_ackmode_and_window_flags() {
    use kiss::{Command, Params, ACKMODE};
    let mut p = Params::default();
    let r = |data: &'static [u8]| kiss::Received { port: 0, command: ACKMODE, data };
    assert_eq!(p.apply(&r(&[])), Command::Unknown);
    assert!(!p.ack_mode);
    assert_eq!(p.apply(&r(&[1])), Command::Set);
    assert!(p.ack_mode);
    assert_eq!(p.apply(&r(&[0])), Command::Set);
    assert!(!p.ack_mode);

    let frame = [0xC0, 0xDB, 0x7E, 1, 2];
    let mut wire = Vec::new();
    kiss::encode_cmd(0, ACKMODE, &frame, |b| wire.push(b));
    assert_eq!(wire[1], 0x0C);
    let mut d: kiss::Decoder = kiss::Decoder::new();
    let mut got = None;
    for &b in &wire {
        if let Some(r) = d.push(b) {
            got = Some((r.port, r.command, r.data.to_vec()));
        }
    }
    assert_eq!(got, Some((0, ACKMODE, frame.to_vec())));

    let p = Params { tx_delay: 30, tx_tail: 0, ..p };
    assert_eq!(p.frame_flags(true, true), (45, 3));
    assert_eq!(p.frame_flags(true, false), (45, 1));
    assert_eq!(p.frame_flags(false, true), (0, 3));
    assert_eq!(p.frame_flags(false, false), (0, 1));
    // 10 ms of tail is still under three flags, so the frame keeps three. 40 ms is six.
    assert_eq!(Params { tx_tail: 1, ..p }.closing_flags(), 3);
    assert_eq!(Params { tx_tail: 4, ..p }.frame_flags(false, true), (0, 6));
}

/// KISS-11: command 5 sets full duplex (and back); a default decoder is a new one.
#[test]
fn kiss_full_duplex_and_default_decoder() {
    use kiss::{Command, Params, Received};
    let mut p = Params::default();
    assert_eq!(p.apply(&Received { port: 0, command: 5, data: &[1] }), Command::Set);
    assert!(p.full_duplex);
    assert_eq!(p.apply(&Received { port: 0, command: 5, data: &[0] }), Command::Set);
    assert!(!p.full_duplex);
    let mut d: kiss::Decoder = kiss::Decoder::default();
    let mut got = Vec::new();
    for b in [kiss::FEND, 0, 7, 8, kiss::FEND] {
        if let Some(r) = d.push(b) {
            got.push(r.data.to_vec());
        }
    }
    assert_eq!(got, vec![vec![7, 8]]);
}

#[test]
fn geo_across_the_date_line_and_south_east() {
    // 179.5 E to 179.5 W at the equator: one degree east, not 359 west.
    let (d, b) = distance_bearing((0, 179_500_000), (0, -179_500_000));
    assert!((d as i64 - 111_195).abs() < 600, "{d}");
    assert_eq!(b, 90);
    // And back the other way.
    let (_, b) = distance_bearing((0, -179_500_000), (0, 179_500_000));
    assert_eq!(b, 270);
    // Due south-east.
    let (_, b) = distance_bearing((0, 0), (-100_000, 100_000));
    assert!((134..=136).contains(&b), "{b}");
}

#[cfg(feature = "ultimeter")]
#[test]
fn ultimeter_fields() {
    use uaprs::parse::{Error, Weather};
    // Gust 10.0 km/h (6 mph), direction 128/256 (180 degrees), 80.0 F, long-term rain unknown, 1019.2 hPa, change
    // unknown, two calibration words, humidity unknown; no date, time, rain today or one-minute wind.
    let Ok(Packet::Weather { timestamp: None, weather, comment }) = parse(b"APRS", b"$ULTW006400800320----27D0----00000000----") else { panic!() };
    let want = Weather { wind_gust: Some(6), wind_dir: Some(180), temp: Some(80), pressure: Some(10192), ..Weather::default() };
    assert_eq!((weather, comment), (want, &b""[..]));
    // A temperature below zero (-10.0 F), and the optional fields present: rain today 0.12 in, wind 10.0 km/h.
    let Ok(Packet::Weather { weather, .. }) = parse(b"APRS", b"$ULTW----0080FF9C----27D0----0000000003E8--------000C0064") else { panic!() };
    assert_eq!((weather.temp, weather.humidity, weather.rain_midnight, weather.wind_speed), (Some(-10), Some(100), Some(12), Some(6)));
    // Too short for the nine fields every report has, or not hexadecimal.
    let full = b"$ULTW006400800320----27D0----00000000----";
    for fields in 0..9 {
        assert_eq!(parse(b"APRS", &full[..5 + 4 * fields]), Err(Error::Short), "{fields} fields");
    }
    assert_eq!(parse(b"APRS", b"$ULTW00G400800320----27D0----00000000----"), Err(Error::Short));
}

#[cfg(feature = "nmea")]
#[test]
fn nmea_symbol_needs_two_characters() {
    let p = position_of(b"GPS", b"$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,");
    assert_eq!((p.table, p.code), (b'/', b'/'));
}

#[test]
fn frame_without_control_and_pid() {
    let f = Frame::new(a("N0CALL"), a("APRS"), &[], b">hi").unwrap();
    let mut buf = [0u8; 64];
    let n = f.write(&mut buf).unwrap();
    assert!(Frame::parse(&buf[..n]).is_some());
    assert!(Frame::parse(&buf[..15]).is_none(), "the address field and one byte");
}

#[test]
fn builder_overlay_speed_rounding_and_plain_message() {
    let r = Report {
        lat: 49_058_333,
        lon: -72_029_167,
        table: b'5',
        code: b'>',
        course_speed: Some((88, 4)),
        altitude: None,
        messaging: false,
        timestamp: None,
        comment: b"",
        ambiguity: 0,
    };
    // An overlay digit goes out as a letter (5 as f) and comes back as the digit. 4 knots is nearer 1.08^21 - 1
    // (4.03) than 1.08^20 - 1 (3.66).
    let mut buf = [0u8; 64];
    let n = build::compressed(&r, &mut buf).unwrap();
    assert_eq!(buf[1], b'f');
    let Packet::Position { pos, .. } = reparse(&buf[..n]) else { panic!() };
    assert_eq!((pos.table, pos.course, pos.speed), (b'5', Some(88), Some(4)));
    assert_eq!((buf[11], buf[12]), (22 + 33, 21 + 33));
    // 1 knot is nearer 1.08^9 - 1 (1.00) than 1.08^10 - 1; past 1.08^89 - 1 (942 knots) the scale ends.
    for (knots, k) in [(1u16, 9u8), (999, 89)] {
        let n = build::compressed(&Report { course_speed: Some((88, knots)), ..r }, &mut buf).unwrap();
        assert_eq!(buf[12], k + 33, "{knots} knots");
        assert!(n > 12);
    }
    // A message without a number.
    let n = build::message(b"N0CALL", b"hi", None, &mut buf).unwrap();
    assert_eq!(&buf[..n], b":N0CALL   :hi");
    // APRS-52.3: a Mic-E comment that would read as a Kenwood radio type goes out behind a space.
    let mut dest = [0u8; 6];
    let kenwood = Report { comment: b">x", ..r };
    let n = build::mic_e(&kenwood, build::MIC_E_EN_ROUTE, &mut dest, &mut buf).unwrap();
    assert!(buf[..n].ends_with(b" >x"));
    for short in 0..n {
        assert_eq!(build::mic_e(&kenwood, build::MIC_E_EN_ROUTE, &mut dest, &mut buf[..short]), None);
    }
}
