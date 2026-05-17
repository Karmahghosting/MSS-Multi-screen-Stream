//! Session codes: encode an IPv4 + port into a short, human-friendly
//! 10-character code formatted as `XXXXX-XXXXX`. Alphabet excludes the
//! visually ambiguous glyphs `0`, `1`, `I`, `O`.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn idx_of(c: char) -> Option<u32> {
    let c = c.to_ascii_uppercase() as u8;
    ALPHABET.iter().position(|&b| b == c).map(|p| p as u32)
}

pub fn encode(ip: Ipv4Addr, port: u16) -> String {
    let o = ip.octets();
    let bytes = [o[0], o[1], o[2], o[3], (port >> 8) as u8, port as u8];

    let mut buf: u64 = 0;
    for &b in &bytes {
        buf = (buf << 8) | b as u64;
    }
    buf <<= 2;

    let mut chars = [b'A'; 10];
    for i in (0..10).rev() {
        chars[i] = ALPHABET[(buf & 0x1f) as usize];
        buf >>= 5;
    }

    let mut out = String::with_capacity(11);
    for (i, &c) in chars.iter().enumerate() {
        if i == 5 {
            out.push('-');
        }
        out.push(c as char);
    }
    out
}

pub fn decode(s: &str) -> Option<(Ipv4Addr, u16)> {
    let cleaned: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .collect();
    if cleaned.len() != 10 {
        return None;
    }

    let mut buf: u64 = 0;
    for c in &cleaned {
        let idx = idx_of(*c)?;
        buf = (buf << 5) | idx as u64;
    }
    buf >>= 2;

    let mut bytes = [0u8; 6];
    for i in (0..6).rev() {
        bytes[i] = (buf & 0xff) as u8;
        buf >>= 8;
    }

    let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
    let port = ((bytes[4] as u16) << 8) | bytes[5] as u16;
    if port == 0 {
        return None;
    }
    Some((ip, port))
}

/// Best-effort local LAN IPv4 (uses the routing table without actually
/// sending packets).
pub fn local_lan_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) => {
            let ip = *a.ip();
            if ip.is_unspecified() || ip.is_loopback() {
                None
            } else {
                Some(ip)
            }
        }
        _ => None,
    }
}
