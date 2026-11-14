//! Text: the style Rune draws a label in, its measured size, the installed font families,
//! and rasterising it for export.
//!
//! Boxes are sized from Rune's own measurement, so a caption's frame always holds the text the
//! canvas draws. Export draws the same face (found through fontdb) with ab_glyph inside them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use rune::tok::{Family, TextStyle};

use crate::model::Style;

static INTER_MEDIUM: &[u8] = include_bytes!("../assets/Inter-Medium.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/Inter-SemiBold.ttf");

/// Line height as a multiple of the font size (Rune's default).
const LINE: f32 = 1.25;

/// `Family::Named` takes a `&'static str`; each family name is leaked once.
fn static_name(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut names = NAMES.get_or_init(Default::default).lock().expect("font names");
    names.entry(name.to_string()).or_insert_with(|| Box::leak(name.to_string().into_boxed_str()))
}

fn weight(st: &Style) -> f32 {
    match (&st.font, st.bold) {
        // Rune bundles Inter up to SemiBold.
        (None, true) => 600.0,
        (None, false) => 500.0,
        (Some(_), true) => 700.0,
        (Some(_), false) => 400.0,
    }
}

/// The style Rune draws `st`'s text in, at `size` points.
pub fn text_style(st: &Style, size: f32) -> TextStyle {
    let t = TextStyle::new(size, weight(st));
    match &st.font {
        Some(name) => t.family(Family::Named(static_name(name))),
        None => t,
    }
}

/// The size of `text` as Rune lays it out: its widest line by its lines.
pub fn text_size(text: &str, st: &Style) -> (f32, f32) {
    let shown = if text.is_empty() { " " } else { text };
    let (w, h) = rune::text::measure(shown, &[], text_style(st, st.font_size), None);
    let lines = shown.split('\n').count() as f32;
    (w.max(st.font_size * 0.5), h.max(lines * st.font_size * LINE))
}

/// One line's width as Rune lays it out, in image pixels.
pub fn line_width(line: &str, st: &Style) -> f32 {
    if line.is_empty() {
        return 0.0;
    }
    rune::text::measure(line, &[], text_style(st, st.font_size), None).0
}

/// A caption box around `text`: its text plus padding on every side.
pub fn box_size(text: &str, st: &Style) -> (f32, f32) {
    let (w, h) = text_size(text, st);
    (w + 2.0 * st.padding, h + 2.0 * st.padding)
}

// ---- Installed families --------------------------------------------------------------------

fn database() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Every installed family name, sorted, without duplicates.
pub fn families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        let mut names: Vec<String> = database().faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    })
}

/// A font file's bytes and the face's index in it.
pub type FontFile = (Arc<Vec<u8>>, u32);

/// A family (`None`: bundled Inter) and whether it's bold.
type FaceKey = (Option<String>, bool);

/// The font file export draws `st`'s text with, and the face's index in it: the installed
/// family, else bundled Inter.
pub fn face_data(st: &Style) -> FontFile {
    static DATA: OnceLock<Mutex<HashMap<FaceKey, FontFile>>> = OnceLock::new();
    let key = (st.font.clone(), st.bold);
    let mut all = DATA.get_or_init(Default::default).lock().expect("font data");
    all.entry(key)
        .or_insert_with(|| {
            let installed = st.font.as_deref().and_then(|name| {
                let query = fontdb::Query {
                    families: &[fontdb::Family::Name(name)],
                    weight: fontdb::Weight(if st.bold { 700 } else { 400 }),
                    ..Default::default()
                };
                let id = database().query(&query)?;
                database().with_face_data(id, |data, index| (Arc::new(data.to_vec()), index))
            });
            installed.unwrap_or_else(|| (Arc::new(if st.bold { INTER_SEMIBOLD } else { INTER_MEDIUM }.to_vec()), 0))
        })
        .clone()
}

/// The bundled Inter, for a PDF when an installed font can't be embedded.
pub fn inter(bold: bool) -> FontFile {
    (Arc::new(if bold { INTER_SEMIBOLD } else { INTER_MEDIUM }.to_vec()), 0)
}

fn face(st: &Style) -> FontArc {
    static FACES: OnceLock<Mutex<HashMap<FaceKey, FontArc>>> = OnceLock::new();
    let key = (st.font.clone(), st.bold);
    let mut faces = FACES.get_or_init(Default::default).lock().expect("faces");
    faces
        .entry(key)
        .or_insert_with(|| {
            let (data, index) = face_data(st);
            ab_glyph::FontVec::try_from_vec_and_index(data.to_vec(), index).map(FontArc::new).unwrap_or_else(|_| {
                FontArc::try_from_slice(if st.bold { INTER_SEMIBOLD } else { INTER_MEDIUM }).expect("bundled font parses")
            })
        })
        .clone()
}

/// Line height as a multiple of the font size, for PDF text.
pub const LINE_HEIGHT: f32 = LINE;

/// Calls `plot(x, y, coverage)` for every pixel of `text` laid out with its top-left at
/// `origin`, in device pixels (`size` is the device font size).
pub fn rasterize(text: &str, st: &Style, size: f32, origin: [f32; 2], mut plot: impl FnMut(i32, i32, f32)) {
    let font = face(st);
    // ab_glyph's scale is the ascent-to-descent height; Rune's size is the em. Convert, or
    // exported text comes out about a fifth smaller than on the canvas.
    let em = font.units_per_em().unwrap_or(1000.0);
    let scale = PxScale::from(size * font.height_unscaled() / em);
    let f = font.as_scaled(scale);
    let line_h = size * LINE;
    // Centre the glyphs' ascent-to-descent span in the line box, as Rune does.
    let pad = (line_h - (f.ascent() - f.descent())) / 2.0;
    // `size` is the device size; Rune measured the line at the image size.
    let k = size / st.font_size.max(f32::EPSILON);
    for (n, line) in text.split('\n').enumerate() {
        let baseline = origin[1] + n as f32 * line_h + pad + f.ascent();
        // Rune sets Inter with its size-specific tracking, so its lines run a little wider than
        // ab_glyph's. Spread the difference between the letters, as tracking does, so the text
        // fills the frame sized from Rune's measurement instead of stopping short of its edge.
        let natural: f32 = {
            let mut w = 0.0;
            let mut prev = None;
            for c in line.chars() {
                let id = f.glyph_id(c);
                if let Some(p) = prev {
                    w += f.kern(p, id);
                }
                w += f.h_advance(id);
                prev = Some(id);
            }
            w
        };
        let count = line.chars().count().max(1) as f32;
        let track = ((line_width(line, st) * k - natural) / count).clamp(-0.2 * size, 0.2 * size);
        let mut x = origin[0];
        let mut prev = None;
        for c in line.chars() {
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                x += f.kern(p, id);
            }
            let glyph = id.with_scale_and_position(scale, point(x, baseline));
            if let Some(outline) = font.outline_glyph(glyph) {
                let b = outline.px_bounds();
                let (bx, by) = (b.min.x as i32, b.min.y as i32);
                outline.draw(|gx, gy, cov| plot(bx + gx as i32, by + gy as i32, cov));
            }
            x += f.h_advance(id) + track;
            prev = Some(id);
        }
    }
}
