#[test]
fn caps() {
    let list = std::env::var("CAPS").unwrap_or_default();
    for path in list.split(',').filter(|s| !s.is_empty()) {
        let d = std::fs::read(path).unwrap();
        let mut i = 12; let (mut rate, mut s) = (0u32, Vec::new());
        while i + 8 <= d.len() {
            let len = u32::from_le_bytes(d[i + 4..i + 8].try_into().unwrap()) as usize;
            let body = &d[i + 8..(i + 8 + len).min(d.len())];
            match &d[i..i + 4] { b"fmt " => rate = u32::from_le_bytes(body[4..8].try_into().unwrap()), b"data" => s = body.chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect(), _ => {} }
            i += 8 + len + (len & 1);
        }
        let mut rx: upocsag::Receiver = upocsag::Receiver::new(rate.max(9600));
        let mut out = Vec::new();
        let f = |m: &upocsag::Message| format!("{}:{}:{}", m.baud, m.address, if m.function == 0 { m.numeric().map(|c| c as char).collect::<String>() } else { m.alpha().map(|c| c as char).filter(|c| !c.is_control()).collect::<String>() });
        for x in s { rx.push(x, |m| out.push(f(m))); }
        rx.flush(|m| out.push(f(m)));
        println!("{}: {:?}", path.rsplit('/').next().unwrap(), out);
    }
}
