//! macOS: `NSPasteboard` (general pasteboard). Safe to use off the main thread.

#![allow(unsafe_code)]

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeHTML,
    NSPasteboardTypePNG, NSPasteboardTypeRTF, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};

use crate::{ClipData, ClipError, Clipboard, within};

#[derive(Debug, Default)]
pub struct MacClipboard;

impl MacClipboard {
    pub fn new() -> Self {
        Self
    }
}

fn pb() -> Retained<NSPasteboard> {
    NSPasteboard::generalPasteboard()
}

fn bytes(d: &NSData) -> Vec<u8> {
    d.to_vec()
}

impl Clipboard for MacClipboard {
    fn sequence(&self) -> u64 {
        u64::try_from(pb().changeCount()).unwrap_or(0)
    }

    fn read(&mut self, limit: u64) -> Result<ClipData, ClipError> {
        let pb = pb();
        let mut out = ClipData::default();
        // SAFETY: the pasteboard type constants are valid static NSStrings.
        let (t_string, t_html, t_rtf, t_png, t_tiff, t_file) = unsafe {
            (
                NSPasteboardTypeString,
                NSPasteboardTypeHTML,
                NSPasteboardTypeRTF,
                NSPasteboardTypePNG,
                NSPasteboardTypeTIFF,
                NSPasteboardTypeFileURL,
            )
        };
        // Files first: when files are copied, Finder also puts their names as text.
        if let Some(items) = pb.pasteboardItems() {
            for item in &items {
                if let Some(url) = item.stringForType(t_file)
                    && let Some(u) = NSURL::URLWithString(&url)
                    && let Some(p) = u.path()
                {
                    out.files.push(PathBuf::from(p.to_string()));
                }
            }
        }
        if !out.files.is_empty() {
            return Ok(out);
        }
        if let Some(s) = pb.stringForType(t_string) {
            let s = s.to_string();
            if within(limit, s.len()) {
                out.text = Some(s);
            }
        }
        if let Some(d) = pb.dataForType(t_html)
            && within(limit, d.len())
        {
            out.html = Some(String::from_utf8_lossy(&bytes(&d)).into_owned());
        }
        if let Some(d) = pb.dataForType(t_rtf)
            && within(limit, d.len())
        {
            out.rtf = Some(bytes(&d));
        }
        if let Some(d) = pb.dataForType(t_png) {
            if within(limit, d.len()) {
                out.png = Some(bytes(&d));
            }
        } else if let Some(tiff) = pb.dataForType(t_tiff)
            && within(limit, tiff.len())
            && let Some(rep) = NSBitmapImageRep::imageRepWithData(&tiff)
        {
            // SAFETY: empty properties dictionary is valid for PNG encoding.
            let png =
                unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) };
            out.png = png.map(|d| bytes(&d));
        }
        Ok(out)
    }

    fn write(&mut self, data: &ClipData) -> Result<(), ClipError> {
        let pb = pb();
        pb.clearContents();
        // SAFETY: valid static type constants.
        let (t_string, t_html, t_rtf, t_png, t_tiff) = unsafe {
            (
                NSPasteboardTypeString,
                NSPasteboardTypeHTML,
                NSPasteboardTypeRTF,
                NSPasteboardTypePNG,
                NSPasteboardTypeTIFF,
            )
        };
        if !data.files.is_empty() {
            let urls: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = data
                .files
                .iter()
                .map(|p| {
                    ProtocolObject::from_retained(NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())))
                })
                .collect();
            let arr = NSArray::from_retained_slice(&urls);
            if !pb.writeObjects(&arr) {
                return Err(ClipError::Os("writing file URLs failed".into()));
            }
            return Ok(());
        }
        if let Some(t) = &data.text {
            pb.setString_forType(&NSString::from_str(t), t_string);
        }
        if let Some(h) = &data.html {
            pb.setData_forType(Some(&NSData::with_bytes(h.as_bytes())), t_html);
        }
        if let Some(r) = &data.rtf {
            pb.setData_forType(Some(&NSData::with_bytes(r)), t_rtf);
        }
        if let Some(p) = &data.png {
            let d = NSData::with_bytes(p);
            pb.setData_forType(Some(&d), t_png);
            // Many Mac apps only read TIFF images.
            if let Some(rep) = NSBitmapImageRep::imageRepWithData(&d)
                && let Some(tiff) = rep.TIFFRepresentation()
            {
                pb.setData_forType(Some(&tiff), t_tiff);
            }
        }
        Ok(())
    }
}
