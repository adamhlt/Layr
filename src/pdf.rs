//! PDF export: the photo as an embedded JPEG, the annotations as vector paths, and their text
//! as real text in embedded fonts, so it stays sharp at any zoom and can be selected, searched
//! and copied.
//!
//! The page works in image pixels with y down (one pixel is 0.75 pt at 100 %, as 96 dpi); a
//! single transform at the top of the content stream maps that onto PDF's points, y up.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use pdf_writer::types::{CidFontType, FontFlags, LineCapStyle, LineJoinStyle, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str};

use crate::font;
use crate::measure::{self, Scale};
use crate::model::{Annotation, BadgeShape, Hatch, Kind, Rgba, Shape, Style};
use crate::render::ExportOptions;
use crate::vector;

/// One embedded face and the glyphs the page used from it.
struct FontUse {
    data: Arc<Vec<u8>>,
    index: u32,
    /// Glyph id → (advance in 1/1000 em, the character it shows).
    glyphs: BTreeMap<u16, (f32, char)>,
    resource: String,
}

struct Page {
    content: Content,
    /// Alpha value (in 1/255) → its graphics state's resource name.
    alphas: BTreeMap<u8, String>,
    fonts: Vec<FontUse>,
    font_index: HashMap<(Option<String>, bool), usize>,
}

fn rgb(c: Rgba) -> (f32, f32, f32) {
    (c.0[0] as f32 / 255.0, c.0[1] as f32 / 255.0, c.0[2] as f32 / 255.0)
}

impl Page {
    fn new() -> Page {
        Page { content: Content::new(), alphas: BTreeMap::new(), fonts: Vec::new(), font_index: HashMap::new() }
    }

    fn alpha(&mut self, a: u8) {
        let n = self.alphas.len();
        let name = self.alphas.entry(a).or_insert_with(|| format!("A{n}")).clone();
        self.content.set_parameters(Name(name.as_bytes()));
    }

    fn fill_color(&mut self, c: Rgba) {
        let (r, g, b) = rgb(c);
        self.content.set_fill_rgb(r, g, b);
        self.alpha(c.0[3]);
    }

    fn stroke_style(&mut self, c: Rgba, width: f32, cap: LineCapStyle) {
        let (r, g, b) = rgb(c);
        self.content.set_stroke_rgb(r, g, b);
        self.content.set_line_width(width);
        self.content.set_line_cap(cap);
        self.content.set_line_join(LineJoinStyle::RoundJoin);
        self.alpha(c.0[3]);
    }

    fn polygon(&mut self, points: &[[f32; 2]]) {
        for (i, p) in points.iter().enumerate() {
            if i == 0 {
                self.content.move_to(p[0], p[1]);
            } else {
                self.content.line_to(p[0], p[1]);
            }
        }
        self.content.close_path();
    }

    fn polyline(&mut self, points: &[[f32; 2]]) {
        for (i, p) in points.iter().enumerate() {
            if i == 0 {
                self.content.move_to(p[0], p[1]);
            } else {
                self.content.line_to(p[0], p[1]);
            }
        }
    }

    fn rounded_rect(&mut self, [x, y, w, h]: [f32; 4], r: f32) {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        if r < 0.01 {
            self.content.rect(x, y, w, h);
            return;
        }
        let k = 0.552_284_8 * r;
        let (rt, bt) = (x + w, y + h);
        let c = &mut self.content;
        c.move_to(x + r, y);
        c.line_to(rt - r, y);
        c.cubic_to(rt - r + k, y, rt, y + r - k, rt, y + r);
        c.line_to(rt, bt - r);
        c.cubic_to(rt, bt - r + k, rt - r + k, bt, rt - r, bt);
        c.line_to(x + r, bt);
        c.cubic_to(x + r - k, bt, x, bt - r + k, x, bt - r);
        c.line_to(x, y + r);
        c.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        c.close_path();
    }

    fn ellipse(&mut self, [x, y, w, h]: [f32; 4]) {
        let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        let (kx, ky) = (0.552_284_8 * rx, 0.552_284_8 * ry);
        let c = &mut self.content;
        c.move_to(cx + rx, cy);
        c.cubic_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry);
        c.cubic_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy);
        c.cubic_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry);
        c.cubic_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy);
        c.close_path();
    }

    /// The face for `st`, embedded once. Font collections can't be embedded as one font
    /// file, so text set in one falls back to Inter.
    fn font_for(&mut self, st: &Style) -> usize {
        let key = (st.font.clone(), st.bold);
        if let Some(&i) = self.font_index.get(&key) {
            return i;
        }
        let (data, index) = font::face_data(st);
        let embeddable = index == 0 && !data.starts_with(b"ttcf") && ttf_parser::Face::parse(&data, 0).is_ok();
        let (data, index) = if embeddable { (data, index) } else { font::inter(st.bold) };
        let i = self.fonts.len();
        self.fonts.push(FontUse { data, index, glyphs: BTreeMap::new(), resource: format!("F{i}") });
        self.font_index.insert(key, i);
        i
    }

    /// `text` in `st`'s face and size with its top-left at `origin`, laid out as the canvas
    /// lays it out (Rune's line height, the line box centred on the glyphs).
    fn text(&mut self, text: &str, st: &Style, origin: [f32; 2], color: Rgba) {
        if text.is_empty() {
            return;
        }
        let fi = self.font_for(st);
        let font = &self.fonts[fi];
        let Ok(face) = ttf_parser::Face::parse(&font.data, font.index) else { return };
        let upem = face.units_per_em() as f32;
        let size = st.font_size;
        let (ascent, descent) = (face.ascender() as f32 / upem * size, face.descender() as f32 / upem * size);
        let line_h = size * font::LINE_HEIGHT;
        let pad = (line_h - (ascent - descent)) / 2.0;
        let mut lines = Vec::new();
        let mut used = Vec::new();
        for line in text.split('\n') {
            let mut bytes = Vec::with_capacity(line.len() * 2);
            let mut natural = 0.0;
            for c in line.chars() {
                let gid = face.glyph_index(c).map_or(0, |g| g.0);
                let advance = face.glyph_hor_advance(ttf_parser::GlyphId(gid)).unwrap_or(0) as f32 / upem * 1000.0;
                natural += advance / 1000.0 * size;
                used.push((gid, advance, c));
                bytes.extend_from_slice(&gid.to_be_bytes());
            }
            // Character spacing that makes the line as wide as Rune measured it, so it fills
            // its frame as on the canvas.
            let count = line.chars().count().max(1) as f32;
            let track = ((font::line_width(line, st) - natural) / count).clamp(-0.2 * size, 0.2 * size);
            lines.push((bytes, track));
        }
        let resource = font.resource.clone();
        for (gid, advance, c) in used {
            self.fonts[fi].glyphs.entry(gid).or_insert((advance, c));
        }
        self.fill_color(color);
        self.content.begin_text();
        self.content.set_font(Name(resource.as_bytes()), size);
        for (n, (bytes, track)) in lines.iter().enumerate() {
            let baseline = origin[1] + n as f32 * line_h + pad + ascent;
            // The page's y runs down; flip the text back upright.
            self.content.set_text_matrix([1.0, 0.0, 0.0, -1.0, origin[0], baseline]);
            self.content.set_char_spacing(*track);
            self.content.show(Str(bytes));
        }
        self.content.end_text();
    }

    fn box_outline(&mut self, a: &Annotation, b: [f32; 4]) {
        if a.kind != Kind::Badge {
            self.rounded_rect(b, a.style.radius);
            return;
        }
        match a.style.badge {
            BadgeShape::Circle => self.ellipse(b),
            shape => match shape.polygon(b) {
                Some(points) => self.polygon(&points),
                None => self.rounded_rect(b, shape.radius(b, a.style.radius)),
            },
        }
    }

    fn hatch(&mut self, a: &Annotation, b: [f32; 4], h: &Hatch) {
        let [x, y, w, hh] = b;
        self.content.save_state();
        self.box_outline(a, b);
        self.content.clip_nonzero();
        self.content.end_path();
        let ang = h.angle_deg.to_radians();
        let (u, n) = ([ang.cos(), ang.sin()], [-ang.sin(), ang.cos()]);
        let c = [x + w / 2.0, y + hh / 2.0];
        let half = (w * w + hh * hh).sqrt() / 2.0 + h.width;
        let mut t = -half;
        while t <= half {
            let o = [c[0] + n[0] * t, c[1] + n[1] * t];
            self.content.move_to(o[0] - u[0] * half, o[1] - u[1] * half);
            self.content.line_to(o[0] + u[0] * half, o[1] + u[1] * half);
            t += h.spacing.max(1.0);
        }
        self.stroke_style(h.color, h.width, LineCapStyle::ButtCap);
        self.content.stroke();
        self.content.restore_state();
    }

    fn fill_then_stroke(&mut self, st: &Style, outline: impl Fn(&mut Page)) {
        if st.fill.0[3] > 0 {
            outline(self);
            self.fill_color(st.fill);
            self.content.fill_nonzero();
        }
        if st.stroke_width > 0.0 {
            outline(self);
            self.stroke_style(st.stroke, st.stroke_width, LineCapStyle::RoundCap);
            self.content.stroke();
        }
    }

    fn annotation(&mut self, a: &Annotation, scale: &Scale) {
        let st = &a.style;
        match &a.shape {
            Shape::Rect { x, y, w, h } => {
                let b = [*x, *y, *w, *h];
                if st.fill.0[3] > 0 {
                    self.box_outline(a, b);
                    self.fill_color(st.fill);
                    self.content.fill_nonzero();
                }
                if let Some(hs) = &st.hatch {
                    self.hatch(a, b, hs);
                }
                if st.stroke_width > 0.0 {
                    self.box_outline(a, b);
                    self.stroke_style(st.stroke, st.stroke_width, LineCapStyle::RoundCap);
                    self.content.stroke();
                }
                if let Some(origin) = a.label_origin() {
                    if a.kind != Kind::Badge {
                        let shadow = (st.font_size / 18.0).max(1.0);
                        self.text(&a.text, st, [origin[0] + shadow, origin[1] + shadow], Rgba::new(0, 0, 0, 180));
                    }
                    self.text(&a.text, st, origin, st.text_color);
                }
            }
            Shape::Polyline { points } => {
                self.polyline(points);
                self.stroke_style(st.stroke, st.stroke_width.max(0.5), LineCapStyle::RoundCap);
                self.content.stroke();
            }
            Shape::Arrow { from, to } if a.kind == Kind::Measure => {
                let w = st.stroke_width.max(0.5);
                for (p, q) in std::iter::once((*from, *to)).chain(measure::ticks(*from, *to, w)) {
                    self.polyline(&[p, q]);
                }
                self.stroke_style(st.stroke, w, LineCapStyle::ButtCap);
                self.content.stroke();
            }
            Shape::Arrow { from, to } => {
                let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
                let len = dx.hypot(dy);
                if len < 0.5 {
                    return;
                }
                let (u, n) = ([dx / len, dy / len], [-dy / len, dx / len]);
                let hl = (st.stroke_width * 4.0).max(10.0).min(len);
                let hw = hl * 0.5;
                let base = [to[0] - u[0] * hl, to[1] - u[1] * hl];
                let shaft = [to[0] - u[0] * hl * 0.7, to[1] - u[1] * hl * 0.7];
                self.polyline(&[*from, shaft]);
                self.stroke_style(st.stroke, st.stroke_width.max(0.5), LineCapStyle::RoundCap);
                self.content.stroke();
                self.polygon(&[*to, [base[0] + n[0] * hw, base[1] + n[1] * hw], [base[0] - n[0] * hw, base[1] - n[1] * hw]]);
                self.fill_color(st.stroke);
                self.content.fill_nonzero();
            }
            Shape::Dot { center, radius } => {
                let r = radius.max(0.5);
                let b = [center[0] - r, center[1] - r, 2.0 * r, 2.0 * r];
                self.fill_then_stroke(st, |p| p.ellipse(b));
            }
            Shape::Text { .. } if a.kind == Kind::Legend => {
                let l = crate::legend::layout(a);
                self.fill_then_stroke(st, |p| p.rounded_rect(l.frame, st.radius));
                if let Some(at) = l.title {
                    self.text(&a.text, st, at, st.text_color);
                }
                for row in &l.rows {
                    self.annotation(&row.sample, scale);
                    self.text(&row.label, st, row.label_origin, st.text_color);
                }
            }
            Shape::Text { pos } => {
                let (w, h) = font::box_size(&a.text, st);
                let b = [pos[0], pos[1], w, h];
                self.fill_then_stroke(st, |p| p.rounded_rect(b, st.radius));
                self.text(&a.text, st, [pos[0] + st.padding, pos[1] + st.padding], st.text_color);
            }
        }
        if let Some(l) = measure::label(a, scale) {
            let ls = vector::label_style(a);
            let b = vector::label_box(&l, &ls);
            self.rounded_rect(b, ls.radius);
            self.fill_color(ls.fill);
            self.content.fill_nonzero();
            self.text(&l.text, &ls, [b[0] + ls.padding, b[1] + ls.padding], ls.text_color);
        }
    }
}

/// A PDF name from a family: letters and digits only.
fn base_font_name(i: usize, data: &[u8], index: u32) -> String {
    let family = ttf_parser::Face::parse(data, index)
        .ok()
        .and_then(|f| f.names().into_iter().find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME).and_then(|n| n.to_string()))
        .unwrap_or_else(|| "Font".into());
    let clean: String = family.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    // A six-letter tag, as subset fonts carry, keeps two embeddings of one family apart.
    format!("LAYRF{}+{clean}", (b'A' + i as u8 % 26) as char)
}

pub fn export(base: &image::RgbaImage, anns: &[Annotation], scale: &Scale, opts: &ExportOptions, path: &Path) -> Result<(), String> {
    let (iw, ih) = base.dimensions();
    let (fw, fh) = (iw as f32, ih as f32);
    // 96 dpi at 100 %: one image pixel is 0.75 pt, times the export scale.
    let k = 0.75 * opts.scale;
    let (pw, ph) = (fw * k, fh * k);

    let mut page = Page::new();
    page.content.save_state();
    // Image pixels, y down, onto PDF points, y up.
    page.content.transform([k, 0.0, 0.0, -k, 0.0, ph]);
    if opts.photo {
        page.content.save_state();
        page.content.transform([fw, 0.0, 0.0, -fh, 0.0, fh]);
        page.content.x_object(Name(b"Photo"));
        page.content.restore_state();
    }
    for a in anns {
        page.annotation(a, scale);
    }
    page.content.restore_state();

    let mut next = 1;
    let mut id = || {
        let r = Ref::new(next);
        next += 1;
        r
    };
    let (catalog, tree, page_id, content_id, photo_id) = (id(), id(), id(), id(), id());
    let alpha_ids: Vec<(String, u8, Ref)> = page.alphas.iter().map(|(a, name)| (name.clone(), *a, id())).collect();
    struct FontRefs {
        type0: Ref,
        cid: Ref,
        descriptor: Ref,
        file: Ref,
        cmap: Ref,
    }
    let font_ids: Vec<FontRefs> =
        page.fonts.iter().map(|_| FontRefs { type0: id(), cid: id(), descriptor: id(), file: id(), cmap: id() }).collect();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page_id]).count(1);
    {
        let mut p = pdf.page(page_id);
        p.parent(tree).media_box(Rect::new(0.0, 0.0, pw, ph)).contents(content_id);
        let mut res = p.resources();
        if opts.photo {
            res.x_objects().pair(Name(b"Photo"), photo_id);
        }
        {
            let mut gs = res.ext_g_states();
            for (name, _, r) in &alpha_ids {
                gs.pair(Name(name.as_bytes()), *r);
            }
        }
        {
            let mut fonts = res.fonts();
            for (f, refs) in page.fonts.iter().zip(&font_ids) {
                fonts.pair(Name(f.resource.as_bytes()), refs.type0);
            }
        }
    }
    pdf.stream(content_id, &page.content.finish());

    if opts.photo {
        let rgb = image::DynamicImage::ImageRgba8(base.clone()).to_rgb8();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, opts.quality.clamp(1, 100))
            .encode_image(&rgb)
            .map_err(|e| e.to_string())?;
        let mut img = pdf.image_xobject(photo_id, &jpeg);
        img.filter(Filter::DctDecode);
        img.width(iw as i32);
        img.height(ih as i32);
        img.color_space().device_rgb();
        img.bits_per_component(8);
        img.finish();
    }

    for (_, a, r) in &alpha_ids {
        let v = *a as f32 / 255.0;
        pdf.ext_graphics(*r).non_stroking_alpha(v).stroking_alpha(v);
    }

    let info = SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 };
    for (i, (f, refs)) in page.fonts.iter().zip(&font_ids).enumerate() {
        let face = ttf_parser::Face::parse(&f.data, f.index).map_err(|e| format!("font: {e}"))?;
        let upem = face.units_per_em() as f32;
        let to_pdf = |v: i16| v as f32 / upem * 1000.0;
        let truetype = face.tables().glyf.is_some();
        let name = base_font_name(i, &f.data, f.index);

        pdf.type0_font(refs.type0)
            .base_font(Name(name.as_bytes()))
            .encoding_predefined(Name(b"Identity-H"))
            .descendant_font(refs.cid)
            .to_unicode(refs.cmap);

        let mut cid = pdf.cid_font(refs.cid);
        cid.subtype(if truetype { CidFontType::Type2 } else { CidFontType::Type0 });
        cid.base_font(Name(name.as_bytes()));
        cid.system_info(info);
        cid.font_descriptor(refs.descriptor);
        cid.default_width(0.0);
        if truetype {
            cid.cid_to_gid_map_predefined(Name(b"Identity"));
        }
        {
            let mut widths = cid.widths();
            for (gid, (advance, _)) in &f.glyphs {
                widths.consecutive(*gid, [*advance]);
            }
        }
        cid.finish();

        let bbox = face.global_bounding_box();
        let mut desc = pdf.font_descriptor(refs.descriptor);
        desc.name(Name(name.as_bytes()))
            .flags(FontFlags::NON_SYMBOLIC)
            .bbox(Rect::new(to_pdf(bbox.x_min), to_pdf(bbox.y_min), to_pdf(bbox.x_max), to_pdf(bbox.y_max)))
            .italic_angle(face.italic_angle())
            .ascent(to_pdf(face.ascender()))
            .descent(to_pdf(face.descender()))
            .cap_height(to_pdf(face.capital_height().unwrap_or(face.ascender())))
            .stem_v(80.0);
        if truetype {
            desc.font_file2(refs.file);
        } else {
            desc.font_file3(refs.file);
        }
        desc.finish();

        let mut file = pdf.stream(refs.file, &f.data);
        if !truetype {
            file.pair(Name(b"Subtype"), Name(b"OpenType"));
        }
        file.finish();

        let mut cmap = UnicodeCmap::new(Name(b"Custom"), info);
        for (gid, (_, c)) in &f.glyphs {
            cmap.pair(*gid, *c);
        }
        pdf.cmap(refs.cmap, &cmap.finish());
    }

    std::fs::write(path, pdf.finish()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdf_carries_its_captions_as_text() {
        let base = image::RgbaImage::from_pixel(200, 100, image::Rgba([30, 40, 50, 255]));
        let a = Annotation {
            id: 1,
            kind: Kind::Text,
            shape: Shape::Text { pos: [10.0, 10.0] },
            style: Kind::Text.default_style(),
            text: "Hello".into(),
            legend: Vec::new(),
        };
        let path = std::env::temp_dir().join("layr-test.pdf");
        export(&base, &[a], &Scale::default(), &ExportOptions { format: crate::render::Format::Pdf, ..Default::default() }, &path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Type0") && text.contains("/ToUnicode") && text.contains("/DCTDecode"), "fonts and photo embedded");
        let _ = std::fs::remove_file(path);
    }
}
