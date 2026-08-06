//! Crockford Base32 (no checksum) for recovery-key display.

/// Crockford alphabet (uppercase). Decoding accepts lower-case and common confusables.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub fn encode(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity((data.len() * 8).div_ceil(5));
    let mut buffer: u64 = 0;
    let mut bits: u32 = 0;
    for &b in data {
        buffer = (buffer << 8) | u64::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let idx = ((buffer >> bits) & 0x1f) as usize;
            out.push(ALPHABET[idx] as char);
        }
    }
    if bits > 0 {
        let idx = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(ALPHABET[idx] as char);
    }
    out
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if cleaned.is_empty() {
        return Some(Vec::new());
    }
    let mut buffer: u64 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(cleaned.len() * 5 / 8);
    for c in cleaned.chars() {
        let v = decode_char(c)?;
        buffer = (buffer << 5) | u64::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn decode_char(c: char) -> Option<u8> {
    // Normalize confusables per Crockford.
    let u = match c {
        'O' | 'o' => '0',
        'I' | 'i' | 'L' | 'l' => '1',
        other => other.to_ascii_uppercase(),
    };
    ALPHABET.iter().position(|&a| a == u as u8).map(|i| i as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_32_bytes() {
        let data: Vec<u8> = (0u8..32).collect();
        let enc = encode(&data);
        let dec = decode(&enc).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn accepts_hyphens_and_case() {
        let data = b"hello-world-padpadpadpadpad!!"; // 28 bytes
        let enc = encode(data);
        let with_hyphen = enc
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-");
        assert_eq!(decode(&with_hyphen.to_ascii_lowercase()).unwrap(), data);
    }

    #[test]
    fn confusable_o_i_l() {
        // '0' encoded value; O/o should decode as 0
        assert_eq!(decode_char('O'), Some(0));
        assert_eq!(decode_char('i'), Some(1));
        assert_eq!(decode_char('L'), Some(1));
    }
}
