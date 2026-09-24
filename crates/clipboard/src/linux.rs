//! Linux clipboard via `arboard` (X11 and Wayland): text, HTML, images, files.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::sync::Mutex;

use crate::{ClipData, ClipError, Clipboard, within};

pub struct LinuxClipboard {
    cb: Mutex<arboard::Clipboard>,
}

impl std::fmt::Debug for LinuxClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinuxClipboard").finish_non_exhaustive()
    }
}

impl LinuxClipboard {
    pub fn new() -> Result<Self, ClipError> {
        Ok(Self { cb: Mutex::new(arboard::Clipboard::new().map_err(|e| ClipError::Os(e.to_string()))?) })
    }
}

fn png_from_rgba(w: usize, h: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, u32::try_from(w).ok()?, u32::try_from(h).ok()?);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(rgba).ok()?;
    }
    Some(out)
}

fn rgba_from_png(png_bytes: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut dec = png::Decoder::new(Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    match r.output_color_type().0 {
        png::ColorType::Rgba => Some((w, h, buf[..w * h * 4].to_vec())),
        png::ColorType::GrayscaleAlpha => {
            Some((w, h, buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()))
        }
        _ => None,
    }
}

impl Clipboard for LinuxClipboard {
    /// Linux has no change counter: a hash of the current contents stands in.
    fn sequence(&self) -> u64 {
        let mut cb = self.cb.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        cb.get_text().ok().hash(&mut h);
        cb.get().file_list().ok().hash(&mut h);
        if let Ok(img) = cb.get_image() {
            (img.width, img.height, img.bytes.len()).hash(&mut h);
            img.bytes.iter().step_by(997).copied().collect::<Vec<u8>>().hash(&mut h);
        }
        h.finish()
    }

    fn read(&mut self, limit: u64) -> Result<ClipData, ClipError> {
        let cb = self.cb.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = ClipData::default();
        if let Ok(files) = cb.get().file_list()
            && !files.is_empty()
        {
            out.files = files;
            return Ok(out);
        }
        out.text = cb.get_text().ok().filter(|t| within(limit, t.len()));
        out.html = cb.get().html().ok().filter(|t| within(limit, t.len()));
        if let Ok(img) = cb.get_image()
            && within(limit, img.bytes.len())
        {
            out.png = png_from_rgba(img.width, img.height, &img.bytes);
        }
        Ok(out)
    }

    fn write(&mut self, data: &ClipData) -> Result<(), ClipError> {
        let cb = self.cb.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let err = |e: arboard::Error| ClipError::Os(e.to_string());
        if !data.files.is_empty() {
            return cb.set().file_list(&data.files).map_err(err);
        }
        if let Some(p) = &data.png
            && let Some((w, h, rgba)) = rgba_from_png(p)
        {
            cb.set_image(arboard::ImageData { width: w, height: h, bytes: Cow::Owned(rgba) }).map_err(err)?;
            return Ok(());
        }
        match (&data.html, &data.text) {
            (Some(html), alt) => cb.set().html(html.as_str(), alt.as_deref()).map_err(err),
            (None, Some(t)) => cb.set_text(t.as_str()).map_err(err),
            (None, None) => Ok(()),
        }
    }
}
