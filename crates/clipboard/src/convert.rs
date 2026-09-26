//! Portable format conversions (tested on every platform):
//! PNG ↔ Windows DIB, Windows "HTML Format" (`CF_HTML`), and `CF_HDROP` file lists.

use std::io::Cursor;

/// Refuse images whose decoded size would exceed this (denial-of-service guard:
/// a few-KB PNG from a peer can declare any size). 8192² covers 8K and
/// multi-monitor screenshots and bounds one decode to 256 MiB (+ the same for the DIB).
const MAX_PIXELS: u64 = 8_192 * 8_192;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConvertError {
    #[error("unsupported or malformed image: {0}")]
    Image(String),
    #[error("malformed clipboard data: {0}")]
    Malformed(&'static str),
}

// ---------------------------------------------------------------------------
// PNG ⇄ DIB
// ---------------------------------------------------------------------------

/// PNG → 32-bit bottom-up `BITMAPINFOHEADER` DIB (what `CF_DIB` expects).
#[allow(clippy::many_single_char_names)]
pub fn png_to_dib(png_bytes: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let mut dec = png::Decoder::new(Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = dec.read_info().map_err(|e| ConvertError::Image(e.to_string()))?;
    let (w, h) = (reader.info().width, reader.info().height);
    if u64::from(w) * u64::from(h) > MAX_PIXELS || w == 0 || h == 0 {
        return Err(ConvertError::Image("image too large".into()));
    }
    let mut buf = vec![0; reader.output_buffer_size().ok_or(ConvertError::Image("size overflow".into()))?];
    let frame = reader.next_frame(&mut buf).map_err(|e| ConvertError::Image(e.to_string()))?;
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight {
        return Err(ConvertError::Image("unsupported bit depth".into()));
    }
    let channels = match color {
        png::ColorType::Rgba => 4usize,
        png::ColorType::GrayscaleAlpha => 2,
        _ => return Err(ConvertError::Image(format!("unsupported colour type {color:?}"))),
    };
    let (wu, hu) = (w as usize, h as usize);
    let stride = frame.line_size;
    let mut out = Vec::with_capacity(40 + wu * hu * 4);
    // BITMAPINFOHEADER
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&i32::try_from(w).map_err(|_| ConvertError::Image("width".into()))?.to_le_bytes());
    out.extend_from_slice(&i32::try_from(h).map_err(|_| ConvertError::Image("height".into()))?.to_le_bytes()); // positive = bottom-up
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bpp
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&u32::try_from(wu * hu * 4).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 dpi
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..hu).rev() {
        let row = &buf[y * stride..y * stride + wu * channels];
        for px in row.chunks_exact(channels) {
            let (r, g, b, a) = if channels == 4 { (px[0], px[1], px[2], px[3]) } else { (px[0], px[0], px[0], px[1]) };
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    Ok(out)
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, ConvertError> {
    let s = b.get(at..at + 4).ok_or(ConvertError::Malformed("short DIB header"))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// `CF_DIB` / `CF_DIBV5` (24 or 32 bpp, `BI_RGB` or `BI_BITFIELDS`) → PNG.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let header = u32_at(dib, 0)? as usize;
    if !(40..=124).contains(&header) {
        return Err(ConvertError::Malformed("unknown DIB header"));
    }
    let w = u32_at(dib, 4)?.cast_signed();
    let h = u32_at(dib, 8)?.cast_signed();
    let bpp = u16::from_le_bytes([dib.get(14).copied().unwrap_or(0), dib.get(15).copied().unwrap_or(0)]);
    let compression = u32_at(dib, 16)?;
    let clr_used = u32_at(dib, 32)? as usize;
    if w <= 0 || h == 0 || u64::from(w.unsigned_abs()) * u64::from(h.unsigned_abs()) > MAX_PIXELS {
        return Err(ConvertError::Image("bad dimensions".into()));
    }
    if !matches!(bpp, 24 | 32) || !matches!(compression, 0 | 3) {
        return Err(ConvertError::Image(format!("unsupported DIB ({bpp} bpp, compression {compression})")));
    }
    // BI_BITFIELDS with a 40-byte header stores 3 masks after the header.
    let masks = if compression == 3 && header == 40 { 12 } else { 0 };
    let offset = header + masks + clr_used * 4;
    let (wu, hu) = (w.unsigned_abs() as usize, h.unsigned_abs() as usize);
    let bytes_pp = usize::from(bpp / 8);
    let stride = (wu * bytes_pp).div_ceil(4) * 4;
    let pixels = dib.get(offset..offset + stride * hu).ok_or(ConvertError::Malformed("pixel data truncated"))?;
    // 32 bpp DIBs often carry alpha = 0 everywhere: treat as opaque then.
    let alpha_used = bpp == 32 && pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    let mut rgba = Vec::with_capacity(wu * hu * 4);
    for row in 0..hu {
        let src_row = if h > 0 { hu - 1 - row } else { row };
        let line = &pixels[src_row * stride..src_row * stride + wu * bytes_pp];
        for px in line.chunks_exact(bytes_pp) {
            let a = if alpha_used { px[3] } else { 255 };
            rgba.extend_from_slice(&[px[2], px[1], px[0], a]);
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, u32::try_from(wu).unwrap_or(0), u32::try_from(hu).unwrap_or(0));
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut writer = enc.write_header().map_err(|e| ConvertError::Image(e.to_string()))?;
        writer.write_image_data(&rgba).map_err(|e| ConvertError::Image(e.to_string()))?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// CF_HTML
// ---------------------------------------------------------------------------

/// Wraps an HTML fragment in the Windows "HTML Format" envelope.
#[must_use]
pub fn html_to_cf_html(fragment: &str) -> Vec<u8> {
    const HEAD: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let pre = "<html><body>\r\n<!--StartFragment-->";
    let post = "<!--EndFragment-->\r\n</body></html>";
    let start_html = HEAD.len();
    let start_frag = start_html + pre.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + post.len();
    let head = format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_frag:010}\r\nEndFragment:{end_frag:010}\r\n"
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(pre.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(post.as_bytes());
    out
}

/// Extracts the fragment from a `CF_HTML` buffer (falls back to the whole HTML).
pub fn cf_html_to_html(data: &[u8]) -> Result<String, ConvertError> {
    let text = String::from_utf8_lossy(data.split(|b| *b == 0).next().unwrap_or(data));
    let field = |name: &str| -> Option<usize> {
        text.lines().find_map(|l| l.strip_prefix(name).and_then(|v| v.trim().parse::<usize>().ok()))
    };
    let bytes = text.as_bytes();
    let (s, e) = match (field("StartFragment:"), field("EndFragment:")) {
        (Some(s), Some(e)) if s <= e && e <= bytes.len() => (s, e),
        _ => match (field("StartHTML:"), field("EndHTML:")) {
            (Some(s), Some(e)) if s <= e && e <= bytes.len() => (s, e),
            _ => return Err(ConvertError::Malformed("CF_HTML offsets")),
        },
    };
    Ok(String::from_utf8_lossy(&bytes[s..e]).into_owned())
}

// ---------------------------------------------------------------------------
// CF_HDROP
// ---------------------------------------------------------------------------

/// `DROPFILES` header + double-NUL-terminated UTF-16 path list.
#[must_use]
pub fn paths_to_hdrop(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes()); // pFiles: offset of the list
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for p in paths {
        for u in p.encode_utf16() {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

pub fn hdrop_to_paths(data: &[u8]) -> Result<Vec<String>, ConvertError> {
    let offset = u32_at(data, 0)? as usize;
    let wide = u32_at(data, 16)? != 0;
    let list = data.get(offset..).ok_or(ConvertError::Malformed("DROPFILES offset"))?;
    let mut out = Vec::new();
    if wide {
        let units: Vec<u16> = list.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        for part in units.split(|u| *u == 0) {
            if part.is_empty() {
                break;
            }
            out.push(String::from_utf16_lossy(part));
        }
    } else {
        for part in list.split(|b| *b == 0) {
            if part.is_empty() {
                break;
            }
            out.push(String::from_utf8_lossy(part).into_owned());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            // red, green / blue, half-transparent white
            w.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 128]).unwrap();
        }
        out
    }

    #[test]
    fn png_dib_roundtrip_keeps_pixels() {
        let dib = png_to_dib(&tiny_png()).unwrap();
        assert_eq!(dib.len(), 40 + 16);
        // bottom-up: first stored row is the bottom row (blue, white)
        assert_eq!(&dib[40..44], &[255, 0, 0, 255]); // blue in BGRA
        let back = dib_to_png(&dib).unwrap();
        let mut dec = png::Decoder::new(Cursor::new(back)).read_info().unwrap();
        let mut buf = vec![0; dec.output_buffer_size().unwrap()];
        dec.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..4], &[255, 0, 0, 255]);
        assert_eq!(&buf[12..16], &[255, 255, 255, 128]);
    }

    #[test]
    fn a_png_declaring_a_huge_size_is_refused_before_decoding() {
        // A peer-supplied PNG header claiming 9000×9000 (~310 MiB decoded) with no pixel data.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 9_000, 9_000);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_chunk(png::chunk::IDAT, &[]).unwrap();
        }
        assert_eq!(png_to_dib(&out), Err(ConvertError::Image("image too large".into())));
    }

    #[test]
    fn dib_24bit_with_padding() {
        // 1x1 24-bit: 3 bytes pixel + 1 byte padding
        let mut dib = Vec::new();
        for v in [40u32, 1, 1] {
            dib.extend_from_slice(&v.to_le_bytes());
        }
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        dib.extend_from_slice(&[10, 20, 30, 0]);
        let png = dib_to_png(&dib).unwrap();
        assert!(!png.is_empty());
    }

    #[test]
    fn hostile_dibs_are_rejected() {
        assert!(dib_to_png(&[]).is_err());
        let mut huge = vec![0u8; 40];
        huge[0] = 40;
        huge[4..8].copy_from_slice(&100_000i32.to_le_bytes());
        huge[8..12].copy_from_slice(&100_000i32.to_le_bytes());
        huge[14] = 32;
        assert!(dib_to_png(&huge).is_err());
    }

    #[test]
    fn cf_html_roundtrip() {
        let frag = "<b>héllo</b> <i>world</i>";
        let cf = html_to_cf_html(frag);
        assert_eq!(cf_html_to_html(&cf).unwrap(), frag);
        assert!(cf_html_to_html(b"garbage").is_err());
    }

    #[test]
    fn hdrop_roundtrip() {
        let paths = vec![r"C:\Users\me\a b.txt".to_owned(), r"D:\日本\x".to_owned()];
        assert_eq!(hdrop_to_paths(&paths_to_hdrop(&paths)).unwrap(), paths);
        assert!(hdrop_to_paths(&[1, 2]).is_err());
    }
}
