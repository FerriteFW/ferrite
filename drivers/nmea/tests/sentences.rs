//! Sentences from real modules' formats: fixes, hemispheres, satellites, bad checksums, overlong lines.

use nmea::Nmea;

/// Feed "$body*CS\r\n" with a correct checksum (or a wrong one when bad).
fn feed(n: &mut Nmea, body: &str, bad: bool) -> usize {
    let cs = body.bytes().fold(0u8, |a, b| a ^ b) ^ bad as u8;
    format!("${body}*{cs:02X}\r\n").bytes().filter(|&c| n.feed(c)).count()
}

#[test]
fn sentences() {
    let mut n = Nmea::new();
    // Tsawwassen-ish: 49 deg 00.6000 N, 123 deg 05.4000 W
    assert_eq!(feed(&mut n, "GNGGA,183412.00,4900.60000,N,12305.40000,W,1,09,0.9,12.3,M,-17.0,M,,", false), 1);
    assert_eq!(n.utc_hms, 183412);
    assert!(n.quality == 1 && n.sats_used == 9 && n.hdop10 == 9 && n.alt_dm == 123);
    assert_eq!(n.lat_e7, 490_100_000); // 49 + 0.6/60 = 49.01
    assert_eq!(n.lon_e7, -1_230_900_000); // 123 + 5.4/60 = 123.09, west
    assert_eq!(&n.last_type, b"GNGGA");

    assert_eq!(feed(&mut n, "GPRMC,183413.00,A,4900.60000,N,12305.40000,W,0.0,,290926,,,A", false), 1);
    assert!(n.valid && n.date_dmy == 290926 && n.utc_hms == 183413);
assert!(n.has_fix());
// Moving: 23.4 knots at 271.5 degrees.
assert_eq!(feed(&mut n, "GNRMC,183413.50,A,4900.60000,N,12305.40000,W,23.4,271.5,290926,,,A", false), 1);
assert!(n.speed_kn10 == 234 && n.course10 == 2715);
    assert_eq!(feed(&mut n, "GPRMC,183414.00,V,,,,,,,290926,,,N", false), 1);
    assert!(!n.valid);
    assert_eq!(n.lat_e7, 490_100_000); // last good position kept

    assert_eq!(feed(&mut n, "GPGSV,3,1,11,01,40,083,46,02,17,308,41,12,07,344,39,14,22,228,45", false), 1);
    assert_eq!(feed(&mut n, "GLGSV,2,1,07,65,20,100,30,66,40,160,35,,,,,,,,", false), 1);
    assert_eq!(n.sats_view, 18);
    assert_eq!(n.nsat, 6); // 4 GP + 2 GL (empty GL slots skipped)
    assert!(n.sat[0].prn == 1 && n.sat[0].snr == 46 && n.sat[3].prn == 14 && n.sat[4].talker == 1);
    assert_eq!(feed(&mut n, "GPGSV,3,2,11,15,10,100,,17,50,200,30", false), 1);
    assert!(n.nsat == 8 && n.sat[6].prn == 15 && n.sat[6].snr == 0 && n.sat[7].snr == 30);
    assert_eq!(feed(&mut n, "GPGSV,3,1,12,01,40,083,46", false), 1);
    assert_eq!(n.sats_view, 19);
    assert_eq!(n.nsat, 3); // GP restarted: 2 GL + 1 GP
    assert_eq!(feed(&mut n, "GAGSV,1,1,01,05,40,083,46,7", false), 1); // NMEA 4.10: trailing signal ID
    assert!(n.nsat == 4 && n.sat[3].prn == 5 && n.sat[3].talker == 2);

    // no fix: GGA quality 0 must not move the position
    assert_eq!(feed(&mut n, "GPGGA,183415.00,,,,,0,00,99.99,,,,,,", false), 1);
    assert!(n.quality == 0 && n.sats_used == 0 && n.lat_e7 == 490_100_000);
    // southern / eastern hemisphere, negative altitude
    assert_eq!(feed(&mut n, "GPGGA,000001,3351.0000,S,15112.0000,E,1,05,1.2,-3.5,M,,,,", false), 1);
    assert!(n.lat_e7 == -338_500_000 && n.lon_e7 == 1_512_000_000 && n.alt_dm == -35);

    let before = n.sentences;
    assert_eq!(feed(&mut n, "GPGGA,1,2,3", true), 0);
    assert!(n.checksum_errors == 1 && n.sentences == before);

    // garbage, missing checksum, overlong line
    for &c in b"\x01\xFF noise $GPGGA,no,star\r\n" {
        n.feed(c);
    }
    assert_eq!(n.checksum_errors, 2);
    n.feed(b'$');
    for _ in 0..200 {
        n.feed(b'A');
    }
    assert_eq!(n.overlong, 1);
    assert_eq!(feed(&mut n, "GPRMC,1,A,4900.6,N,12305.4,W,,,010126,,,A", false), 1); // recovers
}

#[test]
fn default_is_empty_and_fix_comes_from_either_sentence() {
    let mut n = Nmea::default();
    assert!(n.sentences == 0 && n.nsat == 0 && !n.has_fix());
    // GGA quality alone is a fix (RMC never seen).
    assert_eq!(feed(&mut n, "GPGGA,120000,4900.0,N,12300.0,W,2,04,1.0,0.0,M,,,,", false), 1);
    assert!(!n.valid && n.quality == 2 && n.has_fix());
}

/// Bad numbers leave the old values: a second decimal point, a stray character, an empty hemisphere.
#[test]
fn bad_fields_keep_the_last_good_values() {
    let mut n = Nmea::new();
    assert_eq!(feed(&mut n, "GPGGA,120000,4900.60000,N,12305.40000,W,1,05,1.25,10.0,M,,,,", false), 1);
    assert_eq!((n.lat_e7, n.lon_e7, n.hdop10), (490_100_000, -1_230_900_000, 12)); // HDOP truncated to tenths
    assert_eq!(feed(&mut n, "GPGGA,12x000,4900.6.0,N,123x5.4,W,1,05,1a,1.0.0,M,,,,", false), 1);
    assert_eq!((n.utc_hms, n.lat_e7, n.lon_e7, n.alt_dm), (120000, 490_100_000, -1_230_900_000, 100));
    assert_eq!(n.hdop10, 0); // HDOP is not kept: a bad one reads 0
    assert_eq!(feed(&mut n, "GPGGA,120001,1000.0,,2000.0,,1,05,1.0,,M,,,,", false), 1);
    assert_eq!((n.utc_hms, n.lat_e7, n.lon_e7, n.alt_dm), (120001, 490_100_000, -1_230_900_000, 100));
    // RMC: a bad time is not taken; a valid RMC cut short after the latitude, then after the course.
    assert_eq!(feed(&mut n, "GPRMC,bad,A", false), 1);
    assert!(n.valid && n.utc_hms == 120001 && n.lat_e7 == 490_100_000);
    assert_eq!(feed(&mut n, "GPRMC,120002,A,0100.0,S", false), 1);
    assert_eq!((n.lat_e7, n.lon_e7, n.speed_kn10, n.course10), (-10_000_000, -1_230_900_000, 0, 0));
    assert_eq!(feed(&mut n, "GPRMC,120003,A,0100.0,S,00200.0,E,-5.0,360.0", false), 1);
    assert_eq!(n.lon_e7, 20_000_000);
    // A negative speed isn't a number (reads 0); a course of 360.0 is held to 359.9.
    assert_eq!((n.speed_kn10, n.course10), (0, 3599));
    assert_eq!(n.date_dmy, 0); // no date field: none taken
}

#[test]
fn checksums_lowercase_bad_digits_and_nul() {
    let mut n = Nmea::new();
    // The checksum of "GPGGA,1" is 0x4B: lowercase hex is accepted, a non-hex digit is not.
    for &c in b"$GPGGA,1*4b\r\n" {
        n.feed(c);
    }
    assert_eq!((n.sentences, n.utc_hms), (1, 1));
    for &c in b"$GPGGA,1*4G\r\n" {
        n.feed(c);
    }
    assert_eq!((n.sentences, n.checksum_errors), (1, 1));
    // A NUL on the wire ends the line there (as stock's C string): the checksum before it still counts.
    let mut got = 0;
    for &c in b"$GPGGA,1*4B\0junk\n" {
        got += n.feed(c) as u32;
    }
    assert_eq!((got, n.sentences, n.checksum_errors), (1, 2, 1));
    // A star too close to the end: no checksum.
    for &c in b"$GPGGA,1*4\r" {
        n.feed(c);
    }
    assert_eq!(n.checksum_errors, 2);
}

/// Sentences that check out but aren't read: an address that isn't five characters, a type not parsed, a GSV
/// from an unknown talker.
#[test]
fn unread_sentences_count_but_change_nothing() {
    let mut n = Nmea::new();
    assert_eq!(feed(&mut n, "GPGGAX,120000,4900.0,N,12300.0,W,1,05,1.0,1.0,M", false), 1);
    assert_eq!((n.sentences, n.last_type, n.utc_hms), (1, [0; 5], 0));
    assert_eq!(feed(&mut n, "GPGSA,A,3,01,02,03,,,,,,,,,,1.8,0.9,1.5", false), 1);
    assert_eq!(&n.last_type, b"GPGSA");
    assert_eq!(feed(&mut n, "XXGSV,1,1,01,05,40,083,46", false), 1);
    assert_eq!((n.sats_view, n.nsat, &n.last_type), (0, 0, b"XXGSV"));
    // A GSV too short to hold a total or a satellite.
    assert_eq!(feed(&mut n, "GPGSV,1", false), 1);
    assert_eq!((n.sats_view, n.nsat), (0, 0));
}

#[test]
fn satellite_list_limits() {
    let mut n = Nmea::new();
    // PRN 0 and PRN 300 are dropped; an SNR over 99 is held to 99, an empty one reads 0.
    assert_eq!(feed(&mut n, "GPGSV,1,1,200,00,40,083,46,300,40,083,46,07,40,083,120,08,10,010,", false), 1);
    assert_eq!(n.nsat, 2);
    assert!(n.sat[0].prn == 7 && n.sat[0].snr == 99 && n.sat[1].prn == 8 && n.sat[1].snr == 0);
    // Totals in view add across talkers and stop at 255.
    assert_eq!(feed(&mut n, "GLGSV,1,1,100", false), 1);
    assert_eq!(n.sats_view, 255);
    // 24 satellites at most: later ones are dropped until a talker restarts.
    for msg in 2..10 {
        let body = format!("GBGSV,9,{msg},36,{},1,1,30,{},1,1,30,{},1,1,30,{},1,1,30", msg * 4, msg * 4 + 1, msg * 4 + 2, msg * 4 + 3);
        assert_eq!(feed(&mut n, &body, false), 1);
    }
    assert_eq!(n.nsat, 24);
    assert_eq!(n.sat[23].prn, 7 * 4 + 1);
}

/// A number too long for its field is refused, the last good value kept: fifteen digits of latitude scaled to
/// 1e-7 degree overflowed (found by fuzzing).
#[test]
fn overlong_numbers_are_refused() {
    let mut n = Nmea::new();
    assert_eq!(feed(&mut n, "GPRMC,183413.00,A,4900.60000,N,12305.40000,W,0.0,,290926,,,A", false), 1);
    assert_eq!(feed(&mut n, "GPRMC,183413.00,A,999999999999999,N,12305.40000,W,0.0,,290926,,,A", false), 1);
    assert_eq!(n.lat_e7, 490_100_000);
    assert_eq!(feed(&mut n, "GNGGA,000000.00,99968438368563,S,17959.99999,E,2,12,99.9,-420.5,M,,M,,", false), 1);
    assert_eq!(n.lat_e7, 490_100_000, "past i32: kept, not wrapped");
}
