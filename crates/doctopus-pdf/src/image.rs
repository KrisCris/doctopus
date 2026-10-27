//! Pictures: JPEG data is kept as it is, everything else is decoded and
//! written as PNG.

use std::borrow::Cow;

use hayro_interpret::{ImageData, LumaData, RasterImage};
use hayro_syntax::Filter;
use hayro_syntax::object::Stream;

use crate::decode::{self, Problem};

/// Pictures larger than this are not converted.
const MAX_PIXELS: u64 = 64 << 20;

/// A picture ready to be stored as a part.
#[derive(Debug)]
pub(crate) struct Encoded {
    pub(crate) data: Vec<u8>,
    pub(crate) content_type: &'static str,
}

/// The picture behind `image` as JPEG or PNG, or why it could not be
/// converted. Nothing is decoded before its size has been checked against
/// `limit`.
pub(crate) fn encode(image: &RasterImage<'_>, limit: u64) -> Result<Encoded, String> {
    let stream = image.stream();
    let filters = stream.filters();
    if filters.last() == Some(&Filter::DctDecode) && decode::unwraps(stream) {
        let data = unwrapped(stream, limit)?;
        // Some writers put a few bytes in front of the JPEG data.
        let head = &data[..data.len().min(1024)];
        let start = head
            .windows(3)
            .position(|w| w == [0xFF, 0xD8, 0xFF])
            .ok_or("its JPEG data is damaged")?;
        return Ok(Encoded {
            data: data[start..].to_vec(),
            content_type: "image/jpeg",
        });
    }
    let (width, height) = (image.width(), image.height());
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 {
        return Err("it has no pixels".into());
    }
    if pixels > MAX_PIXELS || pixels.saturating_mul(4) > limit {
        return Err(format!(
            "it is too large to convert ({width} x {height} pixels)"
        ));
    }
    check_data(stream, pixels, &filters, limit)?;
    if let Some(mask) = stream.dict().get::<Stream<'_>>(b"SMask") {
        check_data(&mask, pixels, &mask.filters(), limit)?;
    }
    let mut encoded = None;
    image.with_rgba(|data, alpha| encoded = Some(png(data, alpha)), None);
    encoded.unwrap_or_else(|| Err("its data could not be decoded".into()))
}

/// Refuse image data that decodes to far more than its dimensions need,
/// or whose embedded header claims larger dimensions. Fax and JBIG2
/// decoders size their output from the dimensions, within their own caps.
fn check_data(
    stream: &Stream<'_>,
    pixels: u64,
    filters: &[Filter],
    limit: u64,
) -> Result<(), String> {
    let jpx = match filters.last() {
        Some(Filter::JpxDecode) => true,
        Some(Filter::DctDecode) => false,
        Some(Filter::CcittFaxDecode | Filter::Jbig2Decode) => return Ok(()),
        _ => {
            // Up to 16 bits per sample and 4 samples, plus predictor bytes.
            let needed = pixels.saturating_mul(8).saturating_add(1 << 20);
            if decode::decoded_len(stream, needed) > needed {
                return Err("its data decodes to far more than its dimensions need".into());
            }
            return Ok(());
        }
    };
    if !decode::unwraps(stream) {
        return Err("its encoding is not supported".into());
    }
    let data = unwrapped(stream, limit)?;
    let (kind, dimensions) = if jpx {
        ("JPEG 2000", jpx_dimensions(&data))
    } else {
        ("JPEG", jpeg_dimensions(&data))
    };
    let (w, h) = dimensions.ok_or_else(|| format!("its {kind} header is damaged"))?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS.min(pixels.saturating_mul(4)) {
        return Err(format!("its {kind} data is too large ({w} x {h} pixels)"));
    }
    Ok(())
}

/// The JPEG or JPEG 2000 data of `stream`, out of the filters around it.
fn unwrapped<'a>(stream: &Stream<'a>, limit: u64) -> Result<Cow<'a, [u8]>, String> {
    decode::unwrapped(stream, limit).map_err(|problem| match problem {
        Problem::TooLarge => format!("its data expands to more than {} MiB", limit >> 20),
        Problem::Damaged => "its data could not be decoded".into(),
    })
}

/// The size of a JPEG image from its start-of-frame marker.
fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = data.windows(2).take(1024).position(|w| w == [0xFF, 0xD8])? + 2;
    loop {
        let marker = *data.get(at + 1)?;
        if *data.get(at)? != 0xFF {
            return None;
        }
        if marker == 0xFF {
            at += 1;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*data.get(at + 2)?, *data.get(at + 3)?]));
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let frame = data.get(at + 5..at + 9)?;
            let height = u32::from(u16::from_be_bytes([frame[0], frame[1]]));
            let width = u32::from(u16::from_be_bytes([frame[2], frame[3]]));
            return Some((width, height));
        }
        at += 2 + len;
    }
}

/// The size of a JPEG 2000 image from the SIZ marker of its codestream.
fn jpx_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let head = &data[..data.len().min(64 << 10)];
    let at = head
        .windows(4)
        .position(|w| w == [0xFF, 0x4F, 0xFF, 0x51])?;
    let siz = head.get(at + 4..at + 4 + 20)?;
    let read = |i: usize| u32::from_be_bytes([siz[i], siz[i + 1], siz[i + 2], siz[i + 3]]);
    let (width, height, x_offset, y_offset) = (read(4), read(8), read(12), read(16));
    Some((width.checked_sub(x_offset)?, height.checked_sub(y_offset)?))
}

/// Write decoded pixels as PNG. Opaque alpha is dropped, and black and
/// white pictures (scans) are packed to one bit per pixel.
fn png(image: ImageData, alpha: Option<LumaData>) -> Result<Encoded, String> {
    let (width, height) = (image.width(), image.height());
    let (mut color, mut data) = match image {
        ImageData::Rgb(rgb) => (png::ColorType::Rgb, rgb.data),
        ImageData::Luma(luma) => (png::ColorType::Grayscale, luma.data),
    };
    let channels = if color == png::ColorType::Rgb { 3 } else { 1 };
    let pixels = width as usize * height as usize;
    if pixels == 0 || data.len() != pixels * channels {
        return Err("its data could not be decoded".into());
    }
    let alpha = alpha.filter(|a| {
        a.width == width
            && a.height == height
            && a.data.len() == pixels
            && a.data.iter().any(|&v| v != 255)
    });
    let mut depth = png::BitDepth::Eight;
    if let Some(alpha) = alpha {
        data = data
            .chunks_exact(channels)
            .zip(&alpha.data)
            .flat_map(|(px, &a)| px.iter().copied().chain([a]))
            .collect();
        color = if channels == 3 {
            png::ColorType::Rgba
        } else {
            png::ColorType::GrayscaleAlpha
        };
    } else if channels == 1 && data.iter().all(|&v| v == 0 || v == 255) {
        data = pack_bits(&data, width as usize);
        depth = png::BitDepth::One;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    encoder.set_compression(png::Compression::Fast);
    let written = encoder.write_header().and_then(|mut writer| {
        writer
            .write_image_data(&data)
            .and_then(|()| writer.finish())
    });
    written.map_err(|err| format!("it could not be written as PNG: {err}"))?;
    Ok(Encoded {
        data: out,
        content_type: "image/png",
    })
}

/// Pack 0/255 samples to one bit each, rows padded to whole bytes.
fn pack_bits(samples: &[u8], width: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() / 8 + samples.len() / width.max(1));
    for row in samples.chunks(width.max(1)) {
        for chunk in row.chunks(8) {
            let byte = chunk
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &v)| acc | (u8::from(v != 0) << (7 - i)));
            out.push(byte);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_2000_sizes_come_from_the_codestream() {
        let mut siz = vec![0xFF, 0x4F, 0xFF, 0x51, 0x00, 0x29, 0x00, 0x00];
        for value in [640u32, 480, 40, 0] {
            siz.extend_from_slice(&value.to_be_bytes());
        }
        let mut file = b"\0\0\0\x0cjP  \r\n\x87\n....jp2c".to_vec();
        file.extend_from_slice(&siz);
        assert_eq!(jpx_dimensions(&file), Some((600, 480)));
        assert_eq!(jpx_dimensions(&file[..20]), None);
    }

    #[test]
    fn jpeg_sizes_come_from_the_frame_header() {
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x01,
            0xE0, 0x02, 0x80, 0x01,
        ];
        assert_eq!(jpeg_dimensions(&jpeg), Some((640, 480)));
        assert_eq!(jpeg_dimensions(&jpeg[..12]), None);
        assert_eq!(jpeg_dimensions(b"not a jpeg"), None);
    }

    #[test]
    fn bilevel_rows_pack_to_bits() {
        let samples = [255, 0, 0, 255, 255, 255, 255, 255, 0, 255];
        assert_eq!(pack_bits(&samples, 10), [0b1001_1111, 0b0100_0000]);
        assert_eq!(pack_bits(&samples, 5), [0b1001_1000, 0b1110_1000]);
    }
}
