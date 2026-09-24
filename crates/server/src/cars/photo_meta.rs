//! Remove metadata from uploaded car photos without re-encoding them.
//!
//! Phone photos carry EXIF, often including the GPS position where they were
//! taken — usually the owner's home — and every viewer of a shared car can fetch
//! the photo. The pixels are left untouched; only the metadata containers are
//! dropped: JPEG APP1/APP13/COM segments, PNG text/EXIF/time chunks and WebP
//! EXIF/XMP chunks. A file that does not parse cleanly is rejected rather than
//! stored as-is.

use super::ImageKind;

/// Return `bytes` without metadata, or `None` if the container is malformed.
pub fn strip_metadata(kind: ImageKind, bytes: &[u8]) -> Option<Vec<u8>> {
    match kind {
        ImageKind::Jpeg => strip_jpeg(bytes),
        ImageKind::Png => strip_png(bytes),
        ImageKind::Webp => strip_webp(bytes),
    }
}

fn strip_jpeg(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..2]);
    let mut i = 2;
    loop {
        // Markers may be preceded by any number of 0xFF fill bytes.
        if *b.get(i)? != 0xFF {
            return None;
        }
        while *b.get(i)? == 0xFF {
            i += 1;
        }
        let marker = *b.get(i)?;
        i += 1;
        match marker {
            // Start of scan: entropy-coded data follows; keep the rest verbatim.
            0xDA => {
                out.extend_from_slice(&[0xFF, marker]);
                out.extend_from_slice(&b[i..]);
                return Some(out);
            }
            0xD9 => {
                out.extend_from_slice(&[0xFF, marker]);
                return Some(out);
            }
            // Standalone markers without a length.
            0x01 | 0xD0..=0xD7 => out.extend_from_slice(&[0xFF, marker]),
            _ => {
                let len = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
                if len < 2 {
                    return None;
                }
                let seg = b.get(i..i + len)?;
                // APP1 = EXIF/XMP, APP13 = Photoshop/IPTC, COM = comments. APP0
                // (JFIF), APP2 (ICC colour profile) and APP14 (Adobe) are kept:
                // they affect how the image renders.
                let drop = matches!(marker, 0xE1 | 0xED | 0xFE);
                if !drop {
                    out.extend_from_slice(&[0xFF, marker]);
                    out.extend_from_slice(seg);
                }
                i += len;
            }
        }
    }
}

fn strip_png(b: &[u8]) -> Option<Vec<u8>> {
    const SIG: usize = 8;
    if b.len() < SIG {
        return None;
    }
    let mut out = Vec::with_capacity(b.len());
    out.extend_from_slice(&b[..SIG]);
    let mut i = SIG;
    while i < b.len() {
        let len = u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind = b.get(i + 4..i + 8)?;
        let end = i.checked_add(12)?.checked_add(len)?;
        let chunk = b.get(i..end)?;
        let drop = matches!(kind, b"eXIf" | b"tEXt" | b"iTXt" | b"zTXt" | b"tIME");
        if !drop {
            out.extend_from_slice(chunk);
        }
        i = end;
        if kind == b"IEND" {
            return Some(out);
        }
    }
    None
}

fn strip_webp(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let mut body = Vec::with_capacity(b.len());
    body.extend_from_slice(b"WEBP");
    let mut i = 12;
    while i < b.len() {
        let fourcc = b.get(i..i + 4)?;
        let size = u32::from_le_bytes(b.get(i + 4..i + 8)?.try_into().ok()?) as usize;
        let padded = size + (size & 1);
        let end = (i + 8).checked_add(padded)?.min(b.len());
        let chunk = b.get(i..end)?;
        if i + 8 + size > b.len() {
            return None;
        }
        match fourcc {
            b"EXIF" | b"XMP " => {}
            b"VP8X" => {
                let mut c = chunk.to_vec();
                // Clear the "has EXIF" (0x08) and "has XMP" (0x04) flags.
                if let Some(flags) = c.get_mut(8) {
                    *flags &= !(0x08 | 0x04);
                }
                body.extend_from_slice(&c);
            }
            _ => body.extend_from_slice(chunk),
        }
        i = end;
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_drops_exif_and_keeps_jfif_and_scan() {
        let jpeg = [
            &[0xFF, 0xD8][..],
            // APP0 JFIF (kept)
            &[0xFF, 0xE0, 0x00, 0x04, b'J', b'F'],
            // APP1 Exif with a fake GPS payload (dropped)
            &[0xFF, 0xE1, 0x00, 0x06, b'G', b'P', b'S', b'!'],
            // SOS + data + EOI
            &[0xFF, 0xDA, 0x00, 0x02, 0x12, 0x34, 0xFF, 0xD9],
        ]
        .concat();
        let out = strip_metadata(ImageKind::Jpeg, &jpeg).unwrap();
        assert!(!out.windows(3).any(|w| w == b"GPS"));
        assert!(out.windows(2).any(|w| w == b"JF"));
        assert!(out.ends_with(&[0x12, 0x34, 0xFF, 0xD9]));
    }

    #[test]
    fn png_drops_text_chunks() {
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            c.extend_from_slice(&[0, 0, 0, 0]);
            c
        };
        let png = [
            vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            chunk(b"IHDR", &[0; 13]),
            chunk(b"tEXt", b"Location\0home"),
            chunk(b"IDAT", &[1, 2, 3]),
            chunk(b"IEND", &[]),
        ]
        .concat();
        let out = strip_metadata(ImageKind::Png, &png).unwrap();
        assert!(!out.windows(4).any(|w| w == b"home"));
        assert!(out.windows(4).any(|w| w == b"IDAT"));
    }

    #[test]
    fn webp_drops_exif_and_fixes_sizes() {
        let chunk = |fourcc: &[u8], data: &[u8]| {
            let mut c = fourcc.to_vec();
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(data);
            if data.len() % 2 == 1 {
                c.push(0);
            }
            c
        };
        let body = [
            b"WEBP".to_vec(),
            chunk(b"VP8X", &[0x08, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            chunk(b"VP8 ", &[9, 9, 9, 9]),
            chunk(b"EXIF", b"gps-here!"),
        ]
        .concat();
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&(body.len() as u32).to_le_bytes());
        webp.extend_from_slice(&body);

        let out = strip_metadata(ImageKind::Webp, &webp).unwrap();
        assert!(!out.windows(3).any(|w| w == b"gps"));
        let riff = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff + 8, out.len());
        assert_eq!(out[20] & 0x08, 0, "EXIF flag must be cleared");
    }

    #[test]
    fn truncated_files_are_rejected() {
        assert!(strip_metadata(ImageKind::Jpeg, &[0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x10]).is_none());
        assert!(
            strip_metadata(
                ImageKind::Png,
                &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
            )
            .is_none()
        );
    }
}
