//! Draws annotations into a transparent pixmap with tiny-skia. The canvas preview and the
//! exported image both come from here, so what you see is what you export.

use std::path::Path;

use tiny_skia::{Color, FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform};

use crate::measure::{self, Scale};
use crate::model::{Annotation, BadgeShape, Hatch, Kind, Rgba, Shape, Style};
use crate::{font, vector};

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c.0[0], c.0[1], c.0[2], c.0[3]));
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r < 0.01 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01))?));
    }
    let k = 0.552_284_8 * r;
    let (rt, bt) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(rt - r, y);
    pb.cubic_to(rt - r + k, y, rt, y + r - k, rt, y + r);
    pb.line_to(rt, bt - r);
    pb.cubic_to(rt, bt - r + k, rt - r + k, bt, rt - r, bt);
    pb.line_to(x + r, bt);
    pb.cubic_to(x + r - k, bt, x, bt - r + k, x, bt - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn hatch(pm: &mut Pixmap, clip: &tiny_skia::Path, [x, y, w, h]: [f32; 4], hatch: &Hatch, ts: Transform) {
    let Some(mut mask) = Mask::new(pm.width(), pm.height()) else { return };
    mask.fill_path(clip, FillRule::Winding, true, ts);
    let a = hatch.angle_deg.to_radians();
    let (u, n) = ([a.cos(), a.sin()], [-a.sin(), a.cos()]);
    let c = [x + w / 2.0, y + h / 2.0];
    let half = (w * w + h * h).sqrt() / 2.0 + hatch.width;
    let spacing = hatch.spacing.max(1.0);
    let mut pb = PathBuilder::new();
    let mut t = -half;
    while t <= half {
        let o = [c[0] + n[0] * t, c[1] + n[1] * t];
        pb.move_to(o[0] - u[0] * half, o[1] - u[1] * half);
        pb.line_to(o[0] + u[0] * half, o[1] + u[1] * half);
        t += spacing;
    }
    if let Some(path) = pb.finish() {
        let s = Stroke { width: hatch.width, line_cap: LineCap::Butt, ..Stroke::default() };
        pm.stroke_path(&path, &paint(hatch.color), &s, ts, Some(&mask));
    }
}

/// Source-over blends `c` at `coverage` into one pixel.
fn blend(pm: &mut Pixmap, x: i32, y: i32, c: Rgba, coverage: f32) {
    if x < 0 || y < 0 || x >= pm.width() as i32 || y >= pm.height() as i32 {
        return;
    }
    let a = (c.0[3] as f32 / 255.0) * coverage.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = y as usize * pm.width() as usize + x as usize;
    let px = &mut pm.pixels_mut()[i];
    let inv = 1.0 - a;
    let ch = |src: u8, dst: u8| (src as f32 * a + dst as f32 * inv).round().clamp(0.0, 255.0) as u8;
    let out_a = (a * 255.0 + px.alpha() as f32 * inv).round().clamp(0.0, 255.0) as u8;
    let (r, g, b) = (ch(c.0[0], px.red()), ch(c.0[1], px.green()), ch(c.0[2], px.blue()));
    if let Some(p) = PremultipliedColorU8::from_rgba(r.min(out_a), g.min(out_a), b.min(out_a), out_a) {
        *px = p;
    }
}

/// Draws `text` with its top-left at image point `origin`, under image-to-pixmap scale `k`.
fn text(pm: &mut Pixmap, text: &str, st: &Style, origin: [f32; 2], color: Rgba, k: f32) {
    font::rasterize(text, st, st.font_size * k, [origin[0] * k, origin[1] * k], |x, y, cov| blend(pm, x, y, color, cov));
}

fn polygon(points: &[[f32; 2]]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 { pb.move_to(p[0], p[1]) } else { pb.line_to(p[0], p[1]) }
    }
    pb.close();
    pb.finish()
}

/// A box annotation's outline: a badge takes its shape, a rectangle its rounded corners.
fn box_path(a: &Annotation, b: [f32; 4]) -> Option<tiny_skia::Path> {
    let [x, y, w, h] = b;
    if a.kind != Kind::Badge {
        return rounded_rect(x, y, w, h, a.style.radius);
    }
    match a.style.badge {
        BadgeShape::Circle => PathBuilder::from_oval(tiny_skia::Rect::from_xywh(x, y, w.max(0.01), h.max(0.01))?),
        shape => match shape.polygon(b) {
            Some(points) => polygon(&points),
            None => rounded_rect(x, y, w, h, shape.radius(b, a.style.radius)),
        },
    }
}

fn arrow_head(from: [f32; 2], to: [f32; 2], width: f32) -> Option<(tiny_skia::Path, [f32; 2])> {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return None;
    }
    let (u, n) = ([dx / len, dy / len], [-dy / len, dx / len]);
    let hl = (width * 4.0).max(10.0).min(len);
    let hw = hl * 0.5;
    let base = [to[0] - u[0] * hl, to[1] - u[1] * hl];
    let mut pb = PathBuilder::new();
    pb.move_to(to[0], to[1]);
    pb.line_to(base[0] + n[0] * hw, base[1] + n[1] * hw);
    pb.line_to(base[0] - n[0] * hw, base[1] - n[1] * hw);
    pb.close();
    // The shaft stops inside the head so its round cap doesn't poke through the tip.
    Some((pb.finish()?, [to[0] - u[0] * hl * 0.7, to[1] - u[1] * hl * 0.7]))
}

pub fn draw(pm: &mut Pixmap, a: &Annotation, k: f32, scale: &Scale) {
    draw_shape(pm, a, k);
    if let Some(l) = measure::label(a, scale) {
        let st = vector::label_style(a);
        let [x, y, w, h] = vector::label_box(&l, &st);
        if let Some(path) = rounded_rect(x, y, w, h, st.radius) {
            pm.fill_path(&path, &paint(st.fill), FillRule::Winding, Transform::from_scale(k, k), None);
        }
        text(pm, &l.text, &st, [x + st.padding, y + st.padding], st.text_color, k);
    }
}

fn draw_shape(pm: &mut Pixmap, a: &Annotation, k: f32) {
    let ts = Transform::from_scale(k, k);
    let st = &a.style;
    match &a.shape {
        Shape::Rect { x, y, w, h } => {
            let Some(path) = box_path(a, [*x, *y, *w, *h]) else { return };
            if st.fill.0[3] > 0 {
                pm.fill_path(&path, &paint(st.fill), FillRule::Winding, ts, None);
            }
            if let Some(hs) = &st.hatch {
                hatch(pm, &path, [*x, *y, *w, *h], hs, ts);
            }
            if st.stroke_width > 0.0 {
                pm.stroke_path(&path, &paint(st.stroke), &stroke(st.stroke_width), ts, None);
            }
            if let Some(origin) = a.label_origin() {
                if a.kind != Kind::Badge {
                    let shadow = (st.font_size / 18.0).max(1.0);
                    text(pm, &a.text, st, [origin[0] + shadow, origin[1] + shadow], Rgba::new(0, 0, 0, 180), k);
                }
                text(pm, &a.text, st, origin, st.text_color, k);
            }
        }
        Shape::Polyline { points } => {
            let mut pb = PathBuilder::new();
            for (i, p) in points.iter().enumerate() {
                if i == 0 { pb.move_to(p[0], p[1]) } else { pb.line_to(p[0], p[1]) }
            }
            if points.len() == 1 {
                pb.line_to(points[0][0] + 0.01, points[0][1]);
            }
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(st.stroke), &stroke(st.stroke_width.max(0.5)), ts, None);
            }
        }
        Shape::Arrow { from, to } if a.kind == Kind::Measure => {
            let w = st.stroke_width.max(0.5);
            let mut pb = PathBuilder::new();
            for (p, q) in std::iter::once((*from, *to)).chain(measure::ticks(*from, *to, w)) {
                pb.move_to(p[0], p[1]);
                pb.line_to(q[0], q[1]);
            }
            if let Some(path) = pb.finish() {
                let s = Stroke { width: w, line_cap: LineCap::Butt, ..Stroke::default() };
                pm.stroke_path(&path, &paint(st.stroke), &s, ts, None);
            }
        }
        Shape::Arrow { from, to } => {
            let Some((head, shaft_end)) = arrow_head(*from, *to, st.stroke_width) else { return };
            let mut pb = PathBuilder::new();
            pb.move_to(from[0], from[1]);
            pb.line_to(shaft_end[0], shaft_end[1]);
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(st.stroke), &stroke(st.stroke_width.max(0.5)), ts, None);
            }
            pm.fill_path(&head, &paint(st.stroke), FillRule::Winding, ts, None);
        }
        Shape::Dot { center, radius } => {
            let Some(path) = PathBuilder::from_circle(center[0], center[1], radius.max(0.5)) else { return };
            if st.fill.0[3] > 0 {
                pm.fill_path(&path, &paint(st.fill), FillRule::Winding, ts, None);
            }
            if st.stroke_width > 0.0 {
                pm.stroke_path(&path, &paint(st.stroke), &stroke(st.stroke_width), ts, None);
            }
        }
        Shape::Text { .. } if a.kind == Kind::Legend => {
            let l = crate::legend::layout(a);
            let [x, y, w, h] = l.frame;
            if let Some(path) = rounded_rect(x, y, w, h, st.radius) {
                if st.fill.0[3] > 0 {
                    pm.fill_path(&path, &paint(st.fill), FillRule::Winding, ts, None);
                }
                if st.stroke_width > 0.0 {
                    pm.stroke_path(&path, &paint(st.stroke), &stroke(st.stroke_width), ts, None);
                }
            }
            if let Some(at) = l.title {
                text(pm, &a.text, st, at, st.text_color, k);
            }
            for row in &l.rows {
                draw_shape(pm, &row.sample, k);
                text(pm, &row.label, st, row.label_origin, st.text_color, k);
            }
        }
        Shape::Text { pos } => {
            let (w, h) = font::box_size(&a.text, st);
            if let Some(path) = rounded_rect(pos[0], pos[1], w, h, st.radius) {
                if st.fill.0[3] > 0 {
                    pm.fill_path(&path, &paint(st.fill), FillRule::Winding, ts, None);
                }
                if st.stroke_width > 0.0 {
                    pm.stroke_path(&path, &paint(st.stroke), &stroke(st.stroke_width), ts, None);
                }
            }
            text(pm, &a.text, st, [pos[0] + st.padding, pos[1] + st.padding], st.text_color, k);
        }
    }
}

/// Renders every annotation of an `iw × ih` image into a pixmap `k` times its size.
pub fn overlay(anns: &[Annotation], iw: u32, ih: u32, k: f32, scale: &Scale) -> Option<Pixmap> {
    let (w, h) = (((iw as f32 * k).round() as u32).max(1), ((ih as f32 * k).round() as u32).max(1));
    let mut pm = Pixmap::new(w, h)?;
    for a in anns {
        draw(&mut pm, a, k, scale);
    }
    Some(pm)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Pdf,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Png, Format::Jpeg, Format::Webp, Format::Pdf];

    pub fn name(self) -> &'static str {
        match self {
            Format::Png => "PNG",
            Format::Jpeg => "JPEG",
            Format::Webp => "WebP",
            Format::Pdf => "PDF",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Png => &["png"],
            Format::Jpeg => &["jpg", "jpeg"],
            Format::Webp => &["webp"],
            Format::Pdf => &["pdf"],
        }
    }

    /// Whether the export can leave out the photo (JPEG has no transparency).
    pub fn can_drop_photo(self) -> bool {
        self != Format::Jpeg
    }
}

/// Which part of the image is exported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Whole,
    /// The largest rectangle without transparent pixels: no empty edges.
    Filled,
    /// A rectangle the user drew.
    Custom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    pub format: Format,
    /// JPEG quality, 1..=100 (also the photo inside a PDF).
    pub quality: u8,
    /// Output size relative to the photo.
    pub scale: f32,
    /// Draw the photo under the annotations; without it the background is transparent.
    pub photo: bool,
    /// PNG: compress harder for a smaller file.
    pub small_png: bool,
    pub area: Area,
    /// The area the user drew, `[x, y, w, h]` in image pixels.
    pub custom: Option<[f32; 4]>,
}

impl Default for ExportOptions {
    fn default() -> ExportOptions {
        ExportOptions { format: Format::Png, quality: 92, scale: 1.0, photo: true, small_png: false, area: Area::Whole, custom: None }
    }
}

impl ExportOptions {
    pub fn size(&self, (w, h): (u32, u32)) -> (u32, u32) {
        (((w as f32 * self.scale).round() as u32).max(1), ((h as f32 * self.scale).round() as u32).max(1))
    }

    fn keeps_photo(&self) -> bool {
        self.photo || !self.format.can_drop_photo()
    }
}

/// The photo (when kept) scaled to the export size, with the annotations drawn over it.
pub fn composite(base: &image::RgbaImage, anns: &[Annotation], scale: &Scale, opts: &ExportOptions) -> Result<image::RgbaImage, String> {
    let (iw, ih) = base.dimensions();
    let (w, h) = opts.size((iw, ih));
    let mut out = match (opts.keeps_photo(), (w, h) == (iw, ih)) {
        (true, true) => base.clone(),
        (true, false) => image::imageops::resize(base, w, h, image::imageops::FilterType::Lanczos3),
        (false, _) => image::RgbaImage::new(w, h),
    };
    let pm = overlay(anns, iw, ih, opts.scale, scale).ok_or("the image is too large to export")?;
    for (dst, src) in out.pixels_mut().zip(pm.pixels()) {
        let sa = src.alpha() as f32 / 255.0;
        if sa <= 0.0 {
            continue;
        }
        let inv = 1.0 - sa;
        let da = dst.0[3] as f32 / 255.0;
        let oa = sa + da * inv;
        // src is premultiplied; dst is straight.
        let ch = |s: u8, d: u8| ((s as f32 + d as f32 * da * inv) / oa).round().clamp(0.0, 255.0) as u8;
        dst.0 = [ch(src.red(), dst.0[0]), ch(src.green(), dst.0[1]), ch(src.blue(), dst.0[2]), (oa * 255.0).round() as u8];
    }
    Ok(out)
}

/// `[x, y, w, h]` cut to whole pixels inside a `w × h` image; the whole image when nothing is
/// left.
pub fn clamp_rect(r: [f32; 4], (w, h): (u32, u32)) -> [u32; 4] {
    let x0 = r[0].round().clamp(0.0, w as f32) as u32;
    let y0 = r[1].round().clamp(0.0, h as f32) as u32;
    let x1 = (r[0] + r[2]).round().clamp(0.0, w as f32) as u32;
    let y1 = (r[1] + r[3]).round().clamp(0.0, h as f32) as u32;
    if x1 > x0 && y1 > y0 { [x0, y0, x1 - x0, y1 - y0] } else { [0, 0, w, h] }
}

/// Writes the part `rect` (`[x, y, w, h]`, image pixels) of the annotated image to `path` as
/// `opts` asks.
pub fn export(
    base: &image::RgbaImage,
    anns: &[Annotation],
    scale: &Scale,
    opts: &ExportOptions,
    rect: [u32; 4],
    path: &Path,
) -> Result<(), String> {
    use image::ImageEncoder;
    // A crop is the cut photo with the annotations shifted onto it; what hangs over the edge
    // is clipped by the smaller canvas.
    let [x, y, cw, ch] = clamp_rect(rect.map(|v| v as f32), base.dimensions());
    let (cut, shifted);
    let (base, anns) = if [x, y, cw, ch] == [0, 0, base.width(), base.height()] {
        (base, anns)
    } else {
        cut = image::imageops::crop_imm(base, x, y, cw, ch).to_image();
        shifted = anns.iter().map(|a| Annotation { shape: a.translated(-(x as f32), -(y as f32)), ..a.clone() }).collect::<Vec<_>>();
        (&cut, shifted.as_slice())
    };
    if opts.format == Format::Pdf {
        return crate::pdf::export(base, anns, scale, opts, path);
    }
    let img = composite(base, anns, scale, opts)?;
    let (w, h) = img.dimensions();
    let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| e.to_string())?);
    let result = match opts.format {
        Format::Png => {
            use image::codecs::png::{CompressionType, FilterType, PngEncoder};
            let level = if opts.small_png { CompressionType::Best } else { CompressionType::Default };
            PngEncoder::new_with_quality(file, level, FilterType::Adaptive).write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        }
        Format::Jpeg => {
            let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(file, opts.quality.clamp(1, 100)).encode_image(&rgb)
        }
        Format::Webp => {
            image::codecs::webp::WebPEncoder::new_lossless(file).write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        }
        Format::Pdf => unreachable!("handled above"),
    };
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Kind, Shape};

    #[test]
    fn export_paints_annotations_over_the_photo() {
        let base = image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 20, 30, 255]));
        let a = Annotation {
            id: 1,
            kind: Kind::Rect,
            shape: Shape::Rect { x: 8.0, y: 8.0, w: 40.0, h: 40.0 },
            style: Kind::Badge.default_style(),
            text: "1".into(),
            legend: Vec::new(),
        };
        let path = std::env::temp_dir().join("layr-export-test.png");
        export(&base, std::slice::from_ref(&a), &Scale::default(), &ExportOptions::default(), [0, 0, 64, 64], &path).unwrap();
        let out = image::open(&path).unwrap().to_rgba8();
        assert_eq!(out.get_pixel(60, 60).0, [10, 20, 30, 255], "outside stays the photo");
        assert_ne!(out.get_pixel(28, 40).0, [10, 20, 30, 255], "inside is tinted");
        // A crop keeps only its part, with the annotations moved along.
        export(&base, &[a], &Scale::default(), &ExportOptions::default(), [20, 30, 40, 30], &path).unwrap();
        let cut = image::open(&path).unwrap().to_rgba8();
        assert_eq!(cut.dimensions(), (40, 30));
        assert_eq!(out.get_pixel(28, 40), cut.get_pixel(8, 10), "the same spot, moved by the crop");
        assert_eq!(out.get_pixel(59, 59), cut.get_pixel(39, 29));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rects_are_cut_to_the_image() {
        assert_eq!(clamp_rect([-5.0, 10.4, 50.0, 200.0], (100, 80)), [0, 10, 45, 70]);
        assert_eq!(clamp_rect([90.0, 0.0, 30.0, 10.0], (100, 80)), [90, 0, 10, 10]);
        assert_eq!(clamp_rect([120.0, 0.0, 30.0, 10.0], (100, 80)), [0, 0, 100, 80], "nothing left");
    }
}
