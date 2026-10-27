//! Reading one page: the glyphs and pictures its content stream draws.

use std::collections::{HashMap, HashSet};
use std::f64::consts::FRAC_PI_2;
use std::panic::{self, AssertUnwindSafe};

use doctopus_core::{Document, Part, PartId, PartKind, Style};
use hayro_interpret::font::{Glyph as PdfGlyph, OutlineGlyph};
use hayro_interpret::hayro_cmap::BfString;
use hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, GlyphDrawMode, Image, InterpreterCache,
    InterpreterSettings, Paint, PathDrawMode, SoftMask, interpret_page,
};
use hayro_syntax::object::{Dict, Name, Stream};
use hayro_syntax::page::{Page, Resources};
use kurbo::{Affine, BezPath, Point, Rect};

use crate::catalog::{self, PageLink};
use crate::image;
use crate::layout::{Glyph, Mark};

/// Glyphs kept per page at most; the rest of the page is cut off.
const MAX_GLYPHS: usize = 1_000_000;
/// Pictures kept per page at most.
const MAX_IMAGES: usize = 1_000;
/// Pictures drawn smaller than this, in points, are rules and dots.
const MIN_IMAGE: f64 = 6.0;
/// Advance assumed for glyphs whose font gives none, in glyph units.
const DEFAULT_ADVANCE: f64 = 500.0;
/// Fonts and pictures looked at at most when naming a page's fonts.
const MAX_RESOURCES: usize = 10_000;

/// Shared by every page of a document.
#[derive(Debug)]
pub(crate) struct Budget {
    /// Bytes of pictures still allowed.
    pub(crate) image_bytes: u64,
    /// Largest picture or attached file.
    pub(crate) limit: u64,
}

/// What was found on a page besides its marks.
#[derive(Debug, Default)]
pub(crate) struct Report {
    /// Glyphs with text.
    pub(crate) glyphs: usize,
    /// Glyphs whose font maps them to no character.
    pub(crate) unmapped: usize,
    /// Vector paths drawn.
    pub(crate) paths: usize,
    /// The pictures drawn, in order.
    pub(crate) images: Vec<PartId>,
    /// Why pictures were left out.
    pub(crate) skipped: Vec<String>,
    /// Glyphs past [`MAX_GLYPHS`].
    pub(crate) cut: bool,
    /// A panic while interpreting the page, which ended it early.
    pub(crate) failure: Option<String>,
}

/// Interpret `page` and collect what it draws, adding its pictures to
/// `doc`.
pub(crate) fn read<'a>(
    page: &Page<'a>,
    number: u32,
    links: &[PageLink],
    cache: &InterpreterCache<'a>,
    settings: &InterpreterSettings,
    doc: &mut Document,
    budget: &mut Budget,
) -> (Vec<Mark>, Report) {
    let crop = page.intersected_crop_box();
    let crop = Rect::new(crop.x0, crop.y0, crop.x1, crop.y1).abs();
    let mut device = Collector {
        marks: Vec::new(),
        report: Report::default(),
        doc,
        budget,
        number,
        links,
        bounds: crop.inflate(crop.width() * 0.05 + 18.0, crop.height() * 0.05 + 18.0),
        font_names: font_names(page.resources()),
        styles: HashMap::new(),
    };
    let mut context = Context::new(Affine::IDENTITY, crop, cache, page.xref(), settings.clone());
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
        interpret_page(page, &mut context, &mut device);
    }));
    if let Err(payload) = outcome {
        device.report.failure = Some(panic_message(payload.as_ref()));
    }
    (device.marks, device.report)
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".into())
}

struct Collector<'p, 'd> {
    marks: Vec<Mark>,
    report: Report,
    doc: &'d mut Document,
    budget: &'d mut Budget,
    number: u32,
    links: &'p [PageLink],
    /// Glyphs outside of this are not on the page.
    bounds: Rect,
    /// Font names by font, from [`font_names`].
    font_names: HashMap<u128, String>,
    /// Bold and italic by font.
    styles: HashMap<u128, Style>,
}

impl Collector<'_, '_> {
    fn style(&mut self, glyph: &OutlineGlyph) -> Style {
        let key = glyph.font_cache_key();
        if let Some(style) = self.styles.get(&key) {
            return *style;
        }
        let font = glyph.font_data();
        let name = font
            .as_ref()
            .and_then(|f| f.postscript_name.clone())
            .or_else(|| self.font_names.get(&key).cloned())
            .unwrap_or_default()
            .to_ascii_lowercase();
        // The style follows the family: "ABCDEF+Arial-BoldMT", "Arial,Bold".
        let style_name = name.rsplit(['-', ',', '+']).next().unwrap_or("");
        let named =
            |words: &[&str]| name != style_name && words.iter().any(|w| style_name.contains(w));
        let style = Style {
            bold: font
                .as_ref()
                .is_some_and(|f| f.weight.is_some_and(|w| w >= 600))
                || named(&["bold", "black", "heavy", "semibold", "demi"]),
            italic: font.as_ref().is_some_and(|f| f.is_italic) || named(&["italic", "oblique"]),
            ..Style::default()
        };
        self.styles.insert(key, style);
        style
    }

    fn place_image(&mut self, id: PartId) {
        self.report.images.push(id.clone());
        self.marks.push(Mark::Image(id));
    }
}

impl<'a> Device<'a> for Collector<'_, '_> {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}

    fn set_blend_mode(&mut self, _: BlendMode) {}

    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {
        self.report.paths += 1;
    }

    fn push_clip_path(&mut self, _: &ClipPath) {}

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph(
        &mut self,
        glyph: &PdfGlyph<'a>,
        transform: Affine,
        glyph_transform: Affine,
        _: &Paint<'a>,
        _: &GlyphDrawMode,
    ) {
        if self.marks.len() >= MAX_GLYPHS {
            self.report.cut = true;
            return;
        }
        // Glyph space has 1000 units to the em.
        let t = transform * glyph_transform;
        let [a, b, c, d, e, f] = t.as_coeffs();
        let size = c.hypot(d) * 1000.0;
        let scale = a.hypot(b);
        let origin = Point::new(e, f);
        if !(size.is_finite() && scale.is_finite() && size >= 0.5 && scale > 0.0)
            || !self.bounds.contains(origin)
        {
            return;
        }
        let text = match glyph.as_unicode() {
            Some(BfString::Char(c)) => c.to_string(),
            Some(BfString::String(s)) => s,
            // Glyph 0 stands for codes that the font has no glyph for.
            None if matches!(glyph, PdfGlyph::Outline(g) if g.glyph_id().to_u32() == 0) => return,
            None => {
                self.report.unmapped += 1;
                return;
            }
        };
        let text: String = text
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        if text.is_empty() {
            return;
        }
        if !text.trim().is_empty() {
            self.report.glyphs += 1;
        }
        let (advance, style) = match glyph {
            PdfGlyph::Outline(g) => (
                g.advance_width()
                    .map(f64::from)
                    .filter(|w| w.is_finite() && *w > 0.0),
                self.style(g),
            ),
            PdfGlyph::Type3(_) => (None, Style::default()),
        };
        let advance = advance.unwrap_or(DEFAULT_ADVANCE);
        let turn = ((b.atan2(a) / FRAC_PI_2).round() as i64).rem_euclid(4) as u8;
        let (x, y) = turned(turn, e, f);
        let middle = t * Point::new(advance / 2.0, 300.0);
        let link = self
            .links
            .iter()
            .position(|l| l.areas.iter().any(|r| r.contains(middle)));
        self.marks.push(Mark::Glyph(Glyph {
            x,
            y,
            size,
            width: advance * scale,
            turn,
            text,
            style,
            link,
        }));
    }

    fn draw_image(&mut self, image: Image<'a, '_>, transform: Affine) {
        let Image::Raster(raster) = image else {
            return;
        };
        if raster.width() < 3 || raster.height() < 3 {
            return;
        }
        // The transform maps pixels onto the page.
        let pixels = Rect::new(
            0.0,
            0.0,
            f64::from(raster.width()),
            f64::from(raster.height()),
        );
        let area = transform.transform_rect_bbox(pixels);
        if area.width() < MIN_IMAGE || area.height() < MIN_IMAGE {
            return;
        }
        if self.report.images.len() >= MAX_IMAGES {
            self.report
                .skipped
                .push(format!("more than {MAX_IMAGES} pictures are on the page"));
            return;
        }
        let origin = raster.stream().dict().obj_id().map(catalog::origin);
        if let Some(part) = origin.as_deref().and_then(|o| self.doc.part_by_origin(o)) {
            let id = part.id.clone();
            self.place_image(id);
            return;
        }
        let limit = self.budget.limit;
        let encoded = panic::catch_unwind(AssertUnwindSafe(|| image::encode(&raster, limit)))
            .unwrap_or_else(|_| Err("its data is damaged".into()));
        let encoded = match encoded {
            Ok(encoded) if encoded.data.len() as u64 > self.budget.image_bytes => {
                Err("the document's pictures together exceed the size limit".into())
            }
            other => other,
        };
        match encoded {
            Ok(encoded) => {
                self.budget.image_bytes -= encoded.data.len() as u64;
                let mut part = Part::new(PartKind::Image, encoded.data)
                    .with_content_type(encoded.content_type)
                    .with_location(format!("page {}", self.number));
                if let Some(origin) = origin {
                    part = part.with_origin(origin);
                }
                let id = self.doc.add_part(part);
                self.place_image(id);
            }
            Err(reason) => self.report.skipped.push(reason),
        }
    }

    fn pop_clip_path(&mut self) {}

    fn pop_transparency_group(&mut self) {}
}

/// The `/BaseFont` names of the fonts a page and its form pictures use,
/// by the key that hayro gives each font (a hash of its dictionary).
/// Standard fonts come with no font data that tells their style, so it is
/// read from their names.
fn font_names(resources: &Resources<'_>) -> HashMap<u128, String> {
    let mut names = HashMap::new();
    let mut pending: Vec<(Dict<'_>, Dict<'_>)> = Vec::new();
    let mut level = Some(resources);
    while let Some(resources) = level {
        pending.push((resources.fonts.clone(), resources.x_objects.clone()));
        level = resources.parent();
    }
    let mut seen = HashSet::new();
    let mut budget = MAX_RESOURCES;
    while let Some((fonts, x_objects)) = pending.pop() {
        for key in fonts.keys() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                return names;
            }
            if let Some(font) = fonts.get::<Dict<'_>>(&*key)
                && let Some(name) = font.get::<Name<'_>>(b"BaseFont")
            {
                names.insert(font.cache_key(), name.as_str().to_string());
            }
        }
        for key in x_objects.keys() {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                return names;
            }
            let Some(form) = x_objects.get::<Stream<'_>>(&*key) else {
                continue;
            };
            let dict = form.dict();
            if dict
                .get::<Name<'_>>(b"Subtype")
                .is_some_and(|s| s.as_str() == "Form")
                && seen.insert(dict.data().as_ptr())
                && let Some(inner) = dict.get::<Dict<'_>>(b"Resources")
            {
                pending.push((
                    inner.get::<Dict<'_>>(b"Font").unwrap_or_default(),
                    inner.get::<Dict<'_>>(b"XObject").unwrap_or_default(),
                ));
            }
        }
    }
    names
}

/// A point in a frame turned by `turn` quarter turns clockwise, so that
/// text drawn in that direction runs left to right.
fn turned(turn: u8, x: f64, y: f64) -> (f64, f64) {
    match turn {
        1 => (y, -x),
        2 => (-x, -y),
        3 => (-y, x),
        _ => (x, y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turned_text_runs_left_to_right() {
        // Text going up the page: later glyphs have larger y.
        let (a, _) = turned(1, 100.0, 200.0);
        let (b, _) = turned(1, 100.0, 210.0);
        assert!(b > a);
        // The next line is to the right of the page, lower in the frame.
        let (_, first) = turned(1, 100.0, 200.0);
        let (_, second) = turned(1, 112.0, 200.0);
        assert!(second < first);
    }
}
