//! uaprs against Ham::APRS::FAP, field by field, on every vector set in the test data's `aprs/` (found by
//! `ferrite_fuzzkit::testdata_dir`'s rule; `wa8lmf` comes from the testdata repo's `fetch/all.sh`; a set that is
//! missing is skipped).
//!
//! `cargo test --release -p uaprs --test report -- --ignored --nocapture` (`UAPRS_SHOW=1` lists disagreements).

use ferrite_fuzzkit::testdata_file;
use serde_json::Value;
use std::collections::BTreeMap;
use uaprs::ax25::Tnc2;
use uaprs::parse::{Format, MessageKind, Packet, Position, Weather};
use uaprs::parse;


/// FAP writes bytes as JSON strings, one character per byte.
fn bytes(v: &Value) -> Option<Vec<u8>> {
    v.as_str().map(|s| s.chars().map(|c| c as u32 as u8).collect())
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
}

/// Field name -> (ours, FAP's) as comparable text; equal when both agree.
type Fields = BTreeMap<&'static str, (String, String)>;

fn close(a: Option<f64>, b: Option<f64>, tol: f64) -> (String, String, bool) {
    let s = |x: Option<f64>| x.map_or("-".into(), |v| format!("{v:.4}"));
    let ok = match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= tol,
        (None, None) => true,
        _ => false,
    };
    (s(a), s(b), ok)
}

fn put(out: &mut Fields, k: &'static str, (a, b, ok): (String, String, bool)) {
    out.insert(k, if ok { (a.clone(), a) } else { (a, b) });
}

fn text(out: &mut Fields, k: &'static str, ours: Option<Vec<u8>>, fap: Option<Vec<u8>>) {
    let s = |x: Option<Vec<u8>>| x.map_or("-".into(), |v| String::from_utf8_lossy(&v).into_owned());
    // FAP drops control characters and empty comments, and trims whitespace.
    let norm = |x: Option<Vec<u8>>| {
        let mut v: Vec<u8> = x.unwrap_or_default().into_iter().filter(|&c| (0x20..0x7F).contains(&c) || (0x80..0xFF).contains(&c)).collect();
        while v.first().is_some_and(|c| c.is_ascii_whitespace()) {
            v.remove(0);
        }
        while v.last().is_some_and(|c| c.is_ascii_whitespace()) {
            v.pop();
        }
        (!v.is_empty()).then_some(v)
    };
    out.insert(k, (s(norm(ours)), s(norm(fap))));
}

fn position(out: &mut Fields, p: &Position, v: &Value) {
    put(out, "lat", close(Some(p.lat as f64 / 1e6), f(v, "latitude"), 2e-5));
    put(out, "lon", close(Some(p.lon as f64 / 1e6), f(v, "longitude"), 2e-5));
    out.insert(
        "symbol",
        (
            format!("{}{}", p.table as char, p.code as char),
            format!("{}{}", v["symboltable"].as_str().unwrap_or("?"), v["symbolcode"].as_str().unwrap_or("?")),
        ),
    );
    if p.format != Format::Nmea {
        put(out, "ambiguity", close(Some(p.ambiguity as f64), f(v, "posambiguity").or(Some(0.0)), 0.0));
    }
    // FAP: km/h, metres; course 0 for unknown.
    put(out, "course", close(p.course.map(|c| c as f64), f(v, "course").filter(|&c| c != 0.0), 0.0));
    put(out, "speed", close(p.speed.map(|s| s as f64 * 1.852), f(v, "speed"), 1.0));
    put(out, "altitude", close(p.altitude.map(|a| a as f64 * 0.3048), f(v, "altitude"), 1.0));
}

fn weather(out: &mut Fields, w: &Weather, v: &Value) {
    let wx = &v["wx"];
    let mph = |x: Option<u16>| x.map(|x| x as f64 * 0.44704);
    put(out, "wx.wind_dir", close(w.wind_dir.map(|x| x as f64), f(wx, "wind_direction"), 0.0));
    put(out, "wx.wind_speed", close(mph(w.wind_speed), f(wx, "wind_speed"), 0.1));
    put(out, "wx.wind_gust", close(mph(w.wind_gust), f(wx, "wind_gust"), 0.1));
    // Whole °F here; Ultimeter reports tenths, which FAP keeps.
    put(out, "wx.temp", close(w.temp.map(|t| (t as f64 - 32.0) * 5.0 / 9.0), f(wx, "temp"), 0.35));
    put(out, "wx.humidity", close(w.humidity.map(|x| x as f64), f(wx, "humidity"), 0.0));
    put(out, "wx.pressure", close(w.pressure.map(|x| x as f64 / 10.0), f(wx, "pressure"), 0.1));
    let mm = |x: Option<u16>| x.map(|x| x as f64 * 0.254);
    put(out, "wx.rain_1h", close(mm(w.rain_1h), f(wx, "rain_1h"), 0.1));
    put(out, "wx.rain_24h", close(mm(w.rain_24h), f(wx, "rain_24h"), 0.1));
    put(out, "wx.rain_midnight", close(mm(w.rain_midnight), f(wx, "rain_midnight"), 0.1));
}

/// Our parse of one packet, compared with FAP's.
fn compare(v: &Value) -> Fields {
    let raw = (0..v["raw"].as_str().unwrap().len() / 2)
        .map(|i| u8::from_str_radix(&v["raw"].as_str().unwrap()[2 * i..2 * i + 2], 16).unwrap())
        .collect::<Vec<u8>>();
    let mut out = Fields::new();
    let fap_ok = v["ok"].as_i64() == Some(1);
    let fap_type = v["type"].as_str().unwrap_or("-").to_string();
    let Some(t) = Tnc2::split(&raw) else {
        out.insert("type", ("-".into(), if fap_ok { fap_type } else { "-".into() }));
        return out;
    };
    // FAP checks an APRS-IS header too: path entries of 1 to 9 letters, digits and dashes. That is the caller's
    // job here (ax25::Address checks RF addresses), so the test applies the same rule.
    // After qAI (a server trace) it also allows hex server ids of any length.
    let mut after_qai = false;
    let path_ok = t.path.is_empty()
        || t.path.split(|&c| c == b',').all(|d| {
            let d = d.strip_suffix(b"*").unwrap_or(d);
            let ok = (1..=9).contains(&d.len()) && d.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
                || after_qai && !d.is_empty() && d.iter().all(u8::is_ascii_hexdigit);
            after_qai |= d == b"qAI";
            ok
        });
    if !path_ok {
        out.insert("type", ("-".into(), if fap_ok { fap_type } else { "-".into() }));
        return out;
    }
    let dest = t.dest.split(|&c| c == b'-').next().unwrap();
    let mut got = parse(dest, t.info);
    // Third-party traffic: compare what it carries (FAP unwraps it when it can).
    while let Ok(Packet::ThirdParty(inner)) = got {
        if !fap_ok {
            break;
        }
        got = parse(inner.dest.split(|&c| c == b'-').next().unwrap(), inner.info);
    }
    let (kind, p) = match &got {
        Ok(Packet::Position { .. }) => ("location", got.as_ref().ok()),
        Ok(Packet::Message(_)) => ("message", got.as_ref().ok()),
        Ok(Packet::Object { .. }) => ("object", got.as_ref().ok()),
        Ok(Packet::Item { .. }) => ("item", got.as_ref().ok()),
        Ok(Packet::Status { .. }) => ("status", got.as_ref().ok()),
        Ok(Packet::Weather { .. }) => ("wx", got.as_ref().ok()),
        Ok(Packet::Capabilities(_)) => ("capabilities", got.as_ref().ok()),
        Ok(Packet::Telemetry(_)) => ("telemetry", got.as_ref().ok()),
        Ok(Packet::ThirdParty(_)) => ("thirdparty", got.as_ref().ok()),
        Ok(Packet::Other { .. }) => ("-", None),
        Err(_) => ("-", None),
    };
    // Ham::APRS::FAP does not unwrap third-party packets; their contents are vectors of their own.
    let fap_kind = if fap_ok { fap_type.as_str() } else if kind == "thirdparty" { "thirdparty" } else { "-" };
    out.insert("type", (kind.into(), fap_kind.into()));
    let Some(p) = p.filter(|_| kind == fap_kind) else {
        return out;
    };
    match p {
        Packet::Position { pos, messaging, mic_e, weather: wx, comment, .. } => {
            position(&mut out, pos, v);
            // FAP drops the comment and altitude of a weather-symbol station that sends no weather; uaprs keeps
            // them.
            if pos.code == b'_' && wx.is_none() {
                out.remove("altitude");
                out.remove("comment");
                return out;
            }
            // FAP leaves `messaging` unset for Mic-E and NMEA.
            if pos.format != Format::Nmea && pos.format != Format::MicE {
                put(&mut out, "messaging", close(Some(*messaging as u8 as f64), f(v, "messaging"), 0.0));
            }
            if let Some(m) = mic_e {
                out.insert("mic_e", (format!("{:03b}", m.bits), v["mbits"].as_str().unwrap_or("-").into()));
            }
            if let Some(w) = wx {
                weather(&mut out, w, v);
            }
            text(&mut out, "comment", Some(comment.bytes().collect()), bytes(&v["comment"]));
        }
        // FAP keeps what follows positionless weather (software and station type) out of the comment.
        Packet::Weather { weather: w, .. } => weather(&mut out, w, v),
        Packet::Message(m) => {
            text(&mut out, "msg.addressee", Some(m.addressee.to_vec()), bytes(&v["destination"]));
            // FAP 1.21 predates reply-acks and keeps "MM}AA" as the id.
            let tag = m.id.map(|id| match m.reply_ack {
                Some(r) => [id, b"}", r].concat(),
                None => id.to_vec(),
            });
            let (id, ack, rej) = match m.kind {
                MessageKind::Message => (tag, None, None),
                MessageKind::Ack => (None, tag, None),
                MessageKind::Rej => (None, None, tag),
            };
            if m.kind == MessageKind::Message {
                text(&mut out, "msg.text", Some(m.text.to_vec()), bytes(&v["message"]));
            }
            text(&mut out, "msg.id", id, bytes(&v["messageid"]));
            text(&mut out, "msg.ack", ack, bytes(&v["messageack"]));
            text(&mut out, "msg.rej", rej, bytes(&v["messagerej"]));
        }
        Packet::Object { name, live, pos, comment, .. } => {
            text(&mut out, "obj.name", Some(name.to_vec()), bytes(&v["objectname"]));
            put(&mut out, "obj.live", close(Some(*live as u8 as f64), f(v, "alive"), 0.0));
            position(&mut out, pos, v);
            text(&mut out, "comment", Some(comment.bytes().collect()), bytes(&v["comment"]));
        }
        Packet::Item { name, live, pos, comment } => {
            text(&mut out, "obj.name", Some(name.to_vec()), bytes(&v["itemname"]));
            put(&mut out, "obj.live", close(Some(*live as u8 as f64), f(v, "alive"), 0.0));
            position(&mut out, pos, v);
            text(&mut out, "comment", Some(comment.bytes().collect()), bytes(&v["comment"]));
        }
        Packet::Status { text: s, .. } => text(&mut out, "status", Some(s.to_vec()), bytes(&v["status"])),
        _ => {}
    }
    out
}

#[test]
#[ignore]
fn against_fap() {
    let show = std::env::var("UAPRS_SHOW").is_ok();
    let mut results = Vec::new();
    println!("\nuaprs against Ham::APRS::FAP: packets where every compared field agrees, and fields that differ\n");
    for set in ["wa8lmf", "suite", "spec"] {
        let Some(path) = testdata_file(&format!("aprs/{set}.jsonl")) else { continue };
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let (mut n, mut agree) = (0, 0);
        let mut by_field: BTreeMap<&str, (u32, u32)> = BTreeMap::new();
        for line in text.lines() {
            let v: Value = serde_json::from_str(line).unwrap();
            let fields = compare(&v);
            n += 1;
            let mut all = true;
            for (k, (a, b)) in &fields {
                let e = by_field.entry(k).or_default();
                e.0 += 1;
                if a == b {
                    e.1 += 1;
                } else {
                    all = false;
                    if show {
                        let raw: Vec<u8> = (0..v["raw"].as_str().unwrap().len() / 2)
                            .map(|i| u8::from_str_radix(&v["raw"].as_str().unwrap()[2 * i..2 * i + 2], 16).unwrap())
                            .collect();
                        eprintln!("  {set} {k}: ours {a:?}, FAP {b:?}\n      {}", String::from_utf8_lossy(&raw));
                    }
                }
            }
            agree += all as u32;
        }
        println!("{set:<8} {agree} of {n} packets agree");
        for (k, (seen, same)) in &by_field {
            if seen != same {
                println!("    {k:<18} {same} of {seen}");
            }
        }
        results.push(format!("RESULT aprs.{set}.packets {n}"));
        results.push(format!("RESULT aprs.{set}.agree {agree}"));
    }
    println!();
    for r in results {
        println!("{r}");
    }
}
