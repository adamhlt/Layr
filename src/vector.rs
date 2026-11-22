//! The live canvas draws annotations as Rune vector elements (shapes, boxes and text) in image
//! pixel coordinates, inside the zoomed world, so they stay sharp at every zoom. Export
//! rasterises the same geometry with tiny-skia (`render.rs`).

use rune::{El, LineCap, Shape, Transition, canvas, el, text};

use crate::font;
use crate::measure::{self, Scale};
use crate::model::{Annotation, BadgeShape, Hatch, Kind, PANEL, Rgba, Shape as Geo, Style};

/// How much larger than its size in image pixels text is laid out, then scaled back down, at
/// camera zoom `zoom`.
///
/// Rune rasterizes text at the zoom it's seen at, but only up to 4×: past that it draws the
/// glyphs made at 1× magnified, and a small image's labels (a few pixels high) turn blocky.
/// Text laid out `k` times larger under a `1/k` scale is seen at `zoom / k`, kept under 4×. `k`
/// moves in powers of two so it changes rarely.
pub fn text_boost(zoom: f32) -> f32 {
    if zoom <= 2.0 { 1.0 } else { 2.0f32.powf((zoom / 2.0).log2().ceil()) }
}

fn label(s: &str, st: &Style, color: Rgba, origin: [f32; 2], k: f32) -> El {
    let big = Style { font_size: st.font_size * k, ..st.clone() };
    let (w, h) = font::text_size(s, &big);
    // Sized as Rune measured it, plus a hair, so it never wraps.
    let t = text(s).font(font::text_style(&big, big.font_size)).color(color.to_rune()).absolute(origin[0], origin[1]).size(w + 1.0, h);
    if k == 1.0 { t } else { t.origin(0.0, 0.0).scale(1.0 / k) }
}

/// A rounded rectangle as a convex polygon, for clipping hatch lines.
fn rounded_polygon([x, y, w, h]: [f32; 4], r: f32) -> Vec<[f32; 2]> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r < 0.01 {
        return vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    }
    let corners = [([x + w - r, y + r], -90.0f32), ([x + w - r, y + h - r], 0.0), ([x + r, y + h - r], 90.0), ([x + r, y + r], 180.0)];
    let mut pts = Vec::with_capacity(4 * 9);
    for (c, start) in corners {
        for i in 0..=8 {
            let a = (start + 90.0 * i as f32 / 8.0).to_radians();
            pts.push([c[0] + r * a.cos(), c[1] + r * a.sin()]);
        }
    }
    pts
}

/// Clips segment `a`–`b` to convex polygon `poly` (Cyrus–Beck).
fn clip(a: [f32; 2], b: [f32; 2], poly: &[[f32; 2]]) -> Option<([f32; 2], [f32; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    // Orientation decides which side of each edge is inside.
    let area: f32 = poly.iter().zip(poly.iter().cycle().skip(1)).map(|(p, q)| p[0] * q[1] - q[0] * p[1]).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    for (p, q) in poly.iter().zip(poly.iter().cycle().skip(1)) {
        let e = [q[0] - p[0], q[1] - p[1]];
        // Inward normal.
        let n = [-e[1] * sign, e[0] * sign];
        let num = n[0] * (a[0] - p[0]) + n[1] * (a[1] - p[1]);
        let den = n[0] * d[0] + n[1] * d[1];
        if den.abs() < 1e-9 {
            if num < 0.0 {
                return None;
            }
            continue;
        }
        let t = -num / den;
        if den > 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some(([a[0] + d[0] * t0, a[1] + d[1] * t0], [a[0] + d[0] * t1, a[1] + d[1] * t1]))
}

/// Adds `h`'s lines across `bounds`, clipped to convex `poly`, to `shape`.
fn hatch_shape(mut shape: Shape, bounds: [f32; 4], poly: &[[f32; 2]], h: &Hatch) -> Shape {
    let [x, y, w, hh] = bounds;
    let a = h.angle_deg.to_radians();
    let (u, n) = ([a.cos(), a.sin()], [-a.sin(), a.cos()]);
    let c = [x + w / 2.0, y + hh / 2.0];
    let half = (w * w + hh * hh).sqrt() / 2.0 + h.width;
    let spacing = h.spacing.max(1.0);
    let color = h.color.to_rune();

    let mut t = -half;
    while t <= half {
        let o = [c[0] + n[0] * t, c[1] + n[1] * t];
        let p0 = [o[0] - u[0] * half, o[1] - u[1] * half];
        let p1 = [o[0] + u[0] * half, o[1] + u[1] * half];
        if let Some((s0, s1)) = clip(p0, p1, poly) {
            shape = shape.line(s0, s1, h.width, LineCap::Butt, color);
        }
        t += spacing;
    }
    shape
}

/// A box drawn as export draws it: fill inside the path, stroke centred on it.
fn boxed(bounds: [f32; 4], st: &Style, hatch: Option<&Hatch>, iw: f32, ih: f32, out: &mut Vec<El>) {
    let [x, y, w, h] = bounds;
    if st.fill.0[3] > 0 {
        out.push(el().absolute(x, y).size(w.max(0.0), h.max(0.0)).radius(st.radius).fill(st.fill.to_rune()));
    }
    if let Some(hs) = hatch {
        let poly = rounded_polygon(bounds, st.radius);
        out.push(canvas(hatch_shape(Shape::new(), bounds, &poly, hs)).absolute(0.0, 0.0).size(iw, ih));
    }
    if st.stroke_width > 0.0 {
        let sw = st.stroke_width;
        out.push(
            el().absolute(x - sw / 2.0, y - sw / 2.0)
                .size(w + sw, h + sw)
                .radius(if st.radius > 0.0 { st.radius + sw / 2.0 } else { 0.0 })
                .border(sw, st.stroke.to_rune()),
        );
    }
}

fn arrow(from: [f32; 2], to: [f32; 2], width: f32, color: Rgba) -> Shape {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.5 {
        return Shape::new();
    }
    let (u, n) = ([dx / len, dy / len], [-dy / len, dx / len]);
    let hl = (width * 4.0).max(10.0).min(len);
    let hw = hl * 0.5;
    let base = [to[0] - u[0] * hl, to[1] - u[1] * hl];
    let shaft = [to[0] - u[0] * hl * 0.7, to[1] - u[1] * hl * 0.7];
    let c = color.to_rune();
    Shape::new()
        .line(from, shaft, width.max(0.5), LineCap::Round, c)
        .fill(vec![to, [base[0] + n[0] * hw, base[1] + n[1] * hw], [base[0] - n[0] * hw, base[1] - n[1] * hw]], c)
}

/// The style a size label is drawn in: a measure's own, else a dark chip under a box.
pub fn label_style(a: &Annotation) -> Style {
    if a.kind == Kind::Measure {
        return a.style.clone();
    }
    Style { fill: PANEL.with_alpha(210), radius: a.style.padding * 0.6, stroke_width: 0.0, ..a.style.clone() }
}

/// A size label's box: `[x, y, w, h]` around its text, centred on the label's point.
pub fn label_box(l: &measure::Label, st: &Style) -> [f32; 4] {
    let (tw, th) = font::text_size(&l.text, st);
    let (w, h) = (tw + 2.0 * st.padding, th + 2.0 * st.padding);
    [l.center[0] - w / 2.0, l.center[1] - h / 2.0, w, h]
}

fn size_label(l: &measure::Label, a: &Annotation, k: f32, out: &mut Vec<El>) {
    let st = label_style(a);
    let [x, y, w, h] = label_box(l, &st);
    out.push(el().absolute(x, y).size(w, h).radius(st.radius).fill(st.fill.to_rune()));
    out.push(label(&l.text, &st, st.text_color, [x + st.padding, y + st.padding], k));
}

/// A circle, diamond or hexagon badge: fill, hatch and outline from one polygon.
fn badge_polygon(b: [f32; 4], st: &Style, iw: f32, ih: f32, out: &mut Vec<El>) {
    let Some(poly) = st.badge.polygon(b) else { return };
    let mut shape = Shape::new();
    if st.fill.0[3] > 0 {
        shape = shape.fill(poly.clone(), st.fill.to_rune());
    }
    if let Some(hs) = &st.hatch {
        shape = hatch_shape(shape, b, &poly, hs);
    }
    if st.stroke_width > 0.0 {
        shape = shape.stroke_with(poly, true, st.stroke_width, LineCap::Round, st.stroke.to_rune());
    }
    out.push(canvas(shape).absolute(0.0, 0.0).size(iw, ih));
}

/// The elements drawing `a`, in image pixels, for a world `iw × ih` points large.
///
/// Each element is keyed by the annotation and a hash of its style, and has no transition: a
/// style change swaps elements in one frame instead of cross-fading old and new (which flashed
/// the fill when a badge changed shape), and moves and resizes keep their elements.
pub fn annotation(a: &Annotation, iw: f32, ih: f32, scale: &Scale, k: f32) -> Vec<El> {
    let look = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // A new boost swaps the text rather than springing its scale.
        format!("{:?}{:?}{}{:?}{k}", a.kind, a.style, a.text, scale).hash(&mut h);
        h.finish()
    };
    let mut parts = annotation_parts(a, iw, ih, k);
    if let Some(l) = measure::label(a, scale) {
        size_label(&l, a, k, &mut parts);
    }
    parts.into_iter().enumerate().map(|(i, e)| e.key(format!("ann-{}-{i}-{look:x}", a.id)).transition(Transition::none())).collect()
}

fn annotation_parts(a: &Annotation, iw: f32, ih: f32, k: f32) -> Vec<El> {
    let st = &a.style;
    let mut out = Vec::new();
    match &a.shape {
        Geo::Rect { x, y, w, h } => {
            let b = [*x, *y, *w, *h];
            match (a.kind, a.kind == Kind::Badge && !matches!(st.badge, BadgeShape::Rounded | BadgeShape::Square)) {
                (_, true) => badge_polygon(b, st, iw, ih, &mut out),
                (Kind::Badge, false) => {
                    let st = Style { radius: st.badge.radius(b, st.radius), ..st.clone() };
                    boxed(b, &st, st.hatch.as_ref(), iw, ih, &mut out);
                }
                _ => boxed(b, st, st.hatch.as_ref(), iw, ih, &mut out),
            }
            if let Some(origin) = a.label_origin() {
                if a.kind != Kind::Badge {
                    let shadow = (st.font_size / 18.0).max(1.0);
                    out.push(label(&a.text, st, Rgba::new(0, 0, 0, 180), [origin[0] + shadow, origin[1] + shadow], k));
                }
                out.push(label(&a.text, st, st.text_color, origin, k));
            }
        }
        Geo::Polyline { points } => {
            let shape = if points.len() == 1 {
                Shape::new().line(points[0], points[0], st.stroke_width.max(0.5), LineCap::Round, st.stroke.to_rune())
            } else {
                Shape::new().stroke(points.clone(), st.stroke_width.max(0.5), st.stroke.to_rune())
            };
            out.push(canvas(shape).absolute(0.0, 0.0).size(iw, ih));
        }
        Geo::Arrow { from, to } if a.kind == Kind::Measure => {
            let c = st.stroke.to_rune();
            let w = st.stroke_width.max(0.5);
            let [t0, t1] = measure::ticks(*from, *to, w);
            let shape = Shape::new().line(*from, *to, w, LineCap::Butt, c).line(t0.0, t0.1, w, LineCap::Butt, c).line(
                t1.0,
                t1.1,
                w,
                LineCap::Butt,
                c,
            );
            out.push(canvas(shape).absolute(0.0, 0.0).size(iw, ih));
        }
        Geo::Arrow { from, to } => out.push(canvas(arrow(*from, *to, st.stroke_width, st.stroke)).absolute(0.0, 0.0).size(iw, ih)),
        Geo::Dot { center, radius } => {
            let r = radius.max(0.5);
            let mut dot = el().absolute(center[0] - r, center[1] - r).size(2.0 * r, 2.0 * r).radius(r);
            if st.fill.0[3] > 0 {
                dot = dot.fill(st.fill.to_rune());
            }
            out.push(dot);
            if st.stroke_width > 0.0 {
                let sw = st.stroke_width;
                let ro = r + sw / 2.0;
                out.push(el().absolute(center[0] - ro, center[1] - ro).size(2.0 * ro, 2.0 * ro).radius(ro).border(sw, st.stroke.to_rune()));
            }
        }
        Geo::Text { .. } if a.kind == Kind::Legend => {
            let l = crate::legend::layout(a);
            boxed(l.frame, st, None, iw, ih, &mut out);
            if let Some(at) = l.title {
                out.push(label(&a.text, st, st.text_color, at, k));
            }
            for row in &l.rows {
                out.extend(annotation_parts(&row.sample, iw, ih, k));
                out.push(label(&row.label, st, st.text_color, row.label_origin, k));
            }
        }
        Geo::Text { pos } => {
            let (w, h) = font::box_size(&a.text, st);
            boxed([pos[0], pos[1], w, h], st, None, iw, ih, &mut out);
            out.push(label(&a.text, st, st.text_color, [pos[0] + st.padding, pos[1] + st.padding], k));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_never_seen_past_four_times_its_layout_size() {
        for i in 0..400 {
            let zoom = 0.02 * 1.03f32.powi(i);
            let seen = zoom / text_boost(zoom);
            assert!(seen <= 4.0, "zoom {zoom}: seen at {seen}");
            assert!(seen >= 1.0 || text_boost(zoom) == 1.0, "zoom {zoom}: boosted needlessly");
        }
        assert_eq!(text_boost(1.5), 1.0);
        assert_eq!(text_boost(10.0), 8.0);
    }

    #[test]
    fn hatch_lines_stay_inside_the_box() {
        let poly = rounded_polygon([0.0, 0.0, 100.0, 50.0], 0.0);
        let (a, b) = clip([-50.0, 25.0], [150.0, 25.0], &poly).unwrap();
        assert!((a[0] - 0.0).abs() < 1e-3 && (b[0] - 100.0).abs() < 1e-3, "{a:?} {b:?}");
        assert!(clip([-50.0, 80.0], [150.0, 80.0], &poly).is_none());
    }
}
