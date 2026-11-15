//! The document: annotations drawn over an image, in image pixel coordinates.

use crate::font;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Rgba {
        Rgba([r, g, b, a])
    }
    pub const fn with_alpha(self, a: u8) -> Rgba {
        Rgba([self.0[0], self.0[1], self.0[2], a])
    }
    pub fn same_hue(self, other: Rgba) -> bool {
        self.0[..3] == other.0[..3]
    }
    pub fn to_rune(self) -> rune::Color {
        // Rune's colours are linear light.
        let [r, g, b, a] = self.0;
        let d = |c: u8| rune::color::srgb_decode(c as f32 / 255.0);
        rune::Color { r: d(r), g: d(g), b: d(b), a: a as f32 / 255.0 }
    }
}

pub const GREEN: Rgba = Rgba::new(46, 204, 64, 255);
pub const RED: Rgba = Rgba::new(235, 45, 45, 255);
pub const MAGENTA: Rgba = Rgba::new(240, 40, 230, 255);
pub const YELLOW: Rgba = Rgba::new(255, 220, 0, 255);
pub const CYAN: Rgba = Rgba::new(40, 190, 240, 255);
pub const ORANGE: Rgba = Rgba::new(255, 140, 20, 255);
pub const WHITE: Rgba = Rgba::new(255, 255, 255, 255);
pub const BLACK: Rgba = Rgba::new(0, 0, 0, 255);
pub const PANEL: Rgba = Rgba::new(18, 18, 22, 255);

/// The swatches the properties panel offers.
pub const PALETTE: [Rgba; 9] = [GREEN, RED, MAGENTA, YELLOW, CYAN, ORANGE, WHITE, PANEL, BLACK];

#[derive(Clone, Debug, PartialEq)]
pub struct Hatch {
    pub color: Rgba,
    pub spacing: f32,
    pub width: f32,
    pub angle_deg: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub stroke: Rgba,
    /// 0 draws no outline.
    pub stroke_width: f32,
    /// Alpha 0 draws no fill.
    pub fill: Rgba,
    pub radius: f32,
    pub hatch: Option<Hatch>,
    pub font_size: f32,
    pub text_color: Rgba,
    pub padding: f32,
    /// An installed font family, or `None` for the bundled Inter.
    pub font: Option<String>,
    pub bold: bool,
    /// The outline a badge takes.
    pub badge: BadgeShape,
    /// A box shows its real width and height under it.
    pub show_size: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeShape {
    Circle,
    Rounded,
    Square,
    Diamond,
    Hexagon,
}

impl BadgeShape {
    pub const ALL: [(BadgeShape, &'static str); 5] = [
        (BadgeShape::Circle, "Circle"),
        (BadgeShape::Rounded, "Rounded"),
        (BadgeShape::Square, "Square"),
        (BadgeShape::Diamond, "Diamond"),
        (BadgeShape::Hexagon, "Hexagon"),
    ];

    /// The outline inside box `[x, y, w, h]` as a polygon, for the shapes that are one.
    pub fn polygon(self, [x, y, w, h]: [f32; 4]) -> Option<Vec<[f32; 2]>> {
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        match self {
            BadgeShape::Diamond => Some(vec![[cx, y], [x + w, cy], [cx, y + h], [x, cy]]),
            BadgeShape::Hexagon => {
                let q = w / 4.0;
                Some(vec![[x + q, y], [x + w - q, y], [x + w, cy], [x + w - q, y + h], [x + q, y + h], [x, cy]])
            }
            BadgeShape::Circle => Some(
                (0..64)
                    .map(|i| {
                        let a = i as f32 * std::f32::consts::TAU / 64.0;
                        [cx + w / 2.0 * a.cos(), cy + h / 2.0 * a.sin()]
                    })
                    .collect(),
            ),
            BadgeShape::Rounded | BadgeShape::Square => None,
        }
    }

    /// The corner radius of a box-shaped badge.
    pub fn radius(self, [_, _, w, h]: [f32; 4], style_radius: f32) -> f32 {
        match self {
            BadgeShape::Rounded => style_radius.max(w.min(h) * 0.22),
            _ => style_radius,
        }
    }
}

impl Style {
    /// Scales every length, so defaults read the same on a small or a large photo.
    pub fn scaled(mut self, k: f32) -> Style {
        self.stroke_width *= k;
        self.radius *= k;
        self.font_size *= k;
        self.padding *= k;
        if let Some(h) = &mut self.hatch {
            h.spacing *= k;
            h.width *= k;
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Polyline {
        points: Vec<[f32; 2]>,
    },
    Arrow {
        from: [f32; 2],
        to: [f32; 2],
    },
    Dot {
        center: [f32; 2],
        radius: f32,
    },
    /// A caption box whose size follows its text; `pos` is its top-left corner.
    Text {
        pos: [f32; 2],
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Rect,
    Hatch,
    Badge,
    Wire,
    Arrow,
    Dot,
    Text,
    /// A dimension line labelled with its real length.
    Measure,
    /// A framed list of the looks used, each with a description.
    Legend,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Rect => "Rectangle",
            Kind::Hatch => "Hatched area",
            Kind::Badge => "Numbered badge",
            Kind::Wire => "Wire",
            Kind::Arrow => "Arrow",
            Kind::Dot => "Dot",
            Kind::Text => "Caption",
            Kind::Measure => "Measure",
            Kind::Legend => "Legend",
        }
    }

    /// The look a new annotation of this kind starts with (at a 2000 px reference size).
    pub fn default_style(self) -> Style {
        let base = Style {
            stroke: GREEN,
            stroke_width: 4.0,
            fill: GREEN.with_alpha(0),
            radius: 0.0,
            hatch: None,
            font_size: 30.0,
            text_color: WHITE,
            padding: 10.0,
            font: None,
            bold: false,
            badge: BadgeShape::Circle,
            show_size: false,
        };
        match self {
            Kind::Rect => Style { stroke: CYAN, stroke_width: 3.0, radius: 14.0, ..base },
            Kind::Hatch => Style {
                stroke: RED,
                stroke_width: 3.0,
                hatch: Some(Hatch { color: RED.with_alpha(200), spacing: 14.0, width: 2.5, angle_deg: 45.0 }),
                fill: RED.with_alpha(40),
                ..base
            },
            Kind::Badge => Style { fill: GREEN.with_alpha(200), stroke: WHITE, stroke_width: 3.0, font_size: 32.0, bold: true, ..base },
            Kind::Wire => Style { stroke: MAGENTA, stroke_width: 5.0, ..base },
            Kind::Arrow => Style { stroke: GREEN, stroke_width: 4.0, ..base },
            Kind::Dot => Style { stroke: Rgba::new(120, 100, 0, 255), stroke_width: 2.0, fill: YELLOW, ..base },
            Kind::Text => Style { fill: PANEL.with_alpha(200), stroke: GREEN, stroke_width: 3.0, ..base },
            Kind::Legend => Style {
                fill: PANEL.with_alpha(225),
                stroke: WHITE.with_alpha(110),
                stroke_width: 2.0,
                font_size: 26.0,
                padding: 16.0,
                radius: 10.0,
                ..base
            },
            Kind::Measure => {
                Style { stroke: YELLOW, stroke_width: 3.0, fill: PANEL.with_alpha(210), font_size: 24.0, padding: 6.0, radius: 4.0, ..base }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub kind: Kind,
    pub shape: Shape,
    pub style: Style,
    /// A caption's text, a badge's number, the label at the top of a rectangle, or a
    /// legend's title.
    pub text: String,
    /// A legend's rows.
    pub legend: Vec<crate::legend::LegendItem>,
}

/// A point handle, by index into [`Annotation::handles`].
pub type Handle = usize;

fn dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = vx * vx + vy * vy;
    let t = if len2 <= f32::EPSILON { 0.0 } else { (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (a[0] + t * vx, a[1] + t * vy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// Snaps `p` so the segment from `from` runs at a multiple of 45°.
/// `[x, y, w, h]` with the edges in `moves` (left, top, right, bottom) dragged to `p`. With
/// `keep_ratio` the shape keeps its proportions: the opposite corner or edge stays put, and a
/// side handle grows the other direction evenly about the middle.
pub fn resize_rect(o: [f32; 4], moves: [bool; 4], p: [f32; 2], keep_ratio: bool) -> [f32; 4] {
    let [x, y, w, h] = o;
    let [ml, mt, mr, mb] = moves;
    let (mut l, mut t, mut r, mut b) = (x, y, x + w, y + h);
    if ml {
        l = p[0];
    }
    if mr {
        r = p[0];
    }
    if mt {
        t = p[1];
    }
    if mb {
        b = p[1];
    }
    let (horiz, vert) = (ml || mr, mt || mb);
    if keep_ratio && w > 0.0 && h > 0.0 && (horiz || vert) {
        let (sx, sy) = ((r - l).abs() / w, (b - t).abs() / h);
        let s = match (horiz, vert) {
            (true, true) => sx.max(sy),
            (true, false) => sx,
            _ => sy,
        };
        let (nw, nh) = (w * s, h * s);
        // Which way from the fixed edge the dragged one went (away from it when level).
        let side = |v: f32, anchor: f32, away: f32| if v != anchor { (v - anchor).signum() } else { away };
        (l, r) = if ml {
            let a = x + w;
            (a + side(p[0], a, -1.0) * nw, a)
        } else if mr {
            (x, x + side(p[0], x, 1.0) * nw)
        } else {
            let c = x + w / 2.0;
            (c - nw / 2.0, c + nw / 2.0)
        };
        (t, b) = if mt {
            let a = y + h;
            (a + side(p[1], a, -1.0) * nh, a)
        } else if mb {
            (y, y + side(p[1], y, 1.0) * nh)
        } else {
            let c = y + h / 2.0;
            (c - nh / 2.0, c + nh / 2.0)
        };
    }
    [l.min(r), t.min(b), (r - l).abs(), (b - t).abs()]
}

pub fn snap45(from: [f32; 2], p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (p[0] - from[0], p[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        return p;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let a = (dy.atan2(dx) / step).round() * step;
    [from[0] + len * a.cos(), from[1] + len * a.sin()]
}

impl Annotation {
    /// Where a box's label starts: centred in a badge, at the top of a rectangle.
    pub fn label_origin(&self) -> Option<[f32; 2]> {
        let Shape::Rect { x, y, w, h } = self.shape else { return None };
        if self.text.is_empty() {
            return None;
        }
        let (tw, th) = font::text_size(&self.text, &self.style);
        Some(if self.kind == Kind::Badge {
            [x + (w - tw) / 2.0, y + (h - th) / 2.0]
        } else {
            [x + (w - tw) / 2.0, y + self.style.padding * 0.5]
        })
    }

    /// The axis-aligned box the annotation covers, `[x, y, w, h]`.
    pub fn bounds(&self) -> [f32; 4] {
        match &self.shape {
            Shape::Rect { x, y, w, h } => [*x, *y, *w, *h],
            Shape::Polyline { points } => bbox(points),
            Shape::Arrow { from, to } => bbox(&[*from, *to]),
            Shape::Dot { center, radius } => [center[0] - radius, center[1] - radius, radius * 2.0, radius * 2.0],
            Shape::Text { .. } if self.kind == Kind::Legend => crate::legend::layout(self).frame,
            Shape::Text { pos } => {
                let (w, h) = font::box_size(&self.text, &self.style);
                [pos[0], pos[1], w, h]
            }
        }
    }

    /// Whether image point `p` touches the annotation, within `tol` pixels.
    pub fn hit(&self, p: [f32; 2], tol: f32) -> bool {
        let half = self.style.stroke_width / 2.0 + tol;
        match &self.shape {
            Shape::Polyline { points } => points.windows(2).any(|s| dist_to_segment(p, s[0], s[1]) <= half),
            Shape::Arrow { from, to } => dist_to_segment(p, *from, *to) <= half.max(tol * 1.5),
            Shape::Dot { center, radius } => ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt() <= radius + tol,
            Shape::Rect { .. } | Shape::Text { .. } => {
                let [x, y, w, h] = self.bounds();
                p[0] >= x - tol && p[0] <= x + w + tol && p[1] >= y - tol && p[1] <= y + h + tol
            }
        }
    }

    /// Points a drag can move: a box's corners and edge midpoints, a line's vertices.
    pub fn handles(&self) -> Vec<[f32; 2]> {
        match &self.shape {
            Shape::Rect { x, y, w, h } => {
                let (r, b, cx, cy) = (x + w, y + h, x + w / 2.0, y + h / 2.0);
                vec![[*x, *y], [r, *y], [r, b], [*x, b], [cx, *y], [r, cy], [cx, b], [*x, cy]]
            }
            Shape::Polyline { points } => points.clone(),
            Shape::Arrow { from, to } => vec![*from, *to],
            Shape::Dot { center, radius } => vec![[center[0] + radius, center[1]]],
            Shape::Text { .. } => Vec::new(),
        }
    }

    /// Moves handle `i` of `orig` to `p`, writing the result into `self`. `shift` snaps lines
    /// to 45°; `keep_ratio` keeps a box's proportions.
    pub fn drag_handle(&mut self, orig: &Annotation, i: Handle, p: [f32; 2], shift: bool, keep_ratio: bool) {
        self.shape = match &orig.shape {
            Shape::Rect { x, y, w, h } => {
                // Handles: the corners clockwise from the top left, then the edges' middles.
                let moves = match i {
                    0 => [true, true, false, false],
                    1 => [false, true, true, false],
                    2 => [false, false, true, true],
                    3 => [true, false, false, true],
                    4 => [false, true, false, false],
                    5 => [false, false, true, false],
                    6 => [false, false, false, true],
                    _ => [true, false, false, false],
                };
                let [x, y, w, h] = resize_rect([*x, *y, *w, *h], moves, p, keep_ratio);
                Shape::Rect { x, y, w, h }
            }
            Shape::Polyline { points } => {
                let mut points = points.clone();
                let p = match (shift, i.checked_sub(1).and_then(|j| points.get(j))) {
                    (true, Some(prev)) => snap45(*prev, p),
                    _ => p,
                };
                if let Some(q) = points.get_mut(i) {
                    *q = p;
                }
                Shape::Polyline { points }
            }
            Shape::Arrow { from, to } => {
                if i == 0 {
                    Shape::Arrow { from: if shift { snap45(*to, p) } else { p }, to: *to }
                } else {
                    Shape::Arrow { from: *from, to: if shift { snap45(*from, p) } else { p } }
                }
            }
            Shape::Dot { center, .. } => {
                let r = ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt().max(1.0);
                Shape::Dot { center: *center, radius: r }
            }
            Shape::Text { pos } => Shape::Text { pos: *pos },
        };
    }

    pub fn translated(&self, dx: f32, dy: f32) -> Shape {
        let m = |p: &[f32; 2]| [p[0] + dx, p[1] + dy];
        match &self.shape {
            Shape::Rect { x, y, w, h } => Shape::Rect { x: x + dx, y: y + dy, w: *w, h: *h },
            Shape::Polyline { points } => Shape::Polyline { points: points.iter().map(m).collect() },
            Shape::Arrow { from, to } => Shape::Arrow { from: m(from), to: m(to) },
            Shape::Dot { center, radius } => Shape::Dot { center: m(center), radius: *radius },
            Shape::Text { pos } => Shape::Text { pos: m(pos) },
        }
    }
}

pub fn bbox(points: &[[f32; 2]]) -> [f32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    if x0 > x1 {
        return [0.0; 4];
    }
    [x0, y0, x1 - x0, y1 - y0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corner_keeps_the_ratio_with_the_opposite_corner_fixed() {
        // 200 × 100 from (100, 100); its bottom right is (300, 200).
        let o = [100.0, 100.0, 200.0, 100.0];
        let tl = [true, true, false, false];
        assert_eq!(resize_rect(o, tl, [0.0, 90.0], true), [0.0, 50.0, 300.0, 150.0], "the larger change wins");
        assert_eq!(resize_rect(o, tl, [0.0, 90.0], false), [0.0, 90.0, 300.0, 110.0]);
        // Dragged past the fixed corner, the shape flips over and still keeps 2:1.
        assert_eq!(resize_rect(o, tl, [340.0, 220.0], true), [300.0, 200.0, 40.0, 20.0]);
    }

    #[test]
    fn a_side_keeps_the_ratio_about_the_middle() {
        let o = [100.0, 100.0, 200.0, 100.0];
        // The right edge out by 200: twice as wide, so twice as tall, around the same middle.
        assert_eq!(resize_rect(o, [false, false, true, false], [500.0, 0.0], true), [100.0, 50.0, 400.0, 200.0]);
        // The top edge down by 50: half as tall, half as wide, centred.
        assert_eq!(resize_rect(o, [false, true, false, false], [0.0, 150.0], true), [150.0, 150.0, 100.0, 50.0]);
    }

    #[test]
    fn a_circle_badge_stays_round() {
        let o = [10.0, 10.0, 60.0, 60.0];
        let r = resize_rect(o, [false, false, true, true], [150.0, 90.0], true);
        assert_eq!(r[2], r[3]);
        assert_eq!(r, [10.0, 10.0, 140.0, 140.0]);
    }

    fn rect() -> Annotation {
        Annotation {
            id: 1,
            kind: Kind::Rect,
            shape: Shape::Rect { x: 10.0, y: 10.0, w: 100.0, h: 50.0 },
            style: Kind::Rect.default_style(),
            text: String::new(),
            legend: Vec::new(),
        }
    }

    #[test]
    fn dragging_a_corner_past_the_opposite_one_flips_the_box() {
        let orig = rect();
        let mut a = orig.clone();
        a.drag_handle(&orig, 0, [200.0, 100.0], false, false);
        assert_eq!(a.shape, Shape::Rect { x: 110.0, y: 60.0, w: 90.0, h: 40.0 });
    }

    #[test]
    fn hits_follow_the_shape() {
        let wire = Annotation { shape: Shape::Polyline { points: vec![[0.0, 0.0], [100.0, 0.0]] }, kind: Kind::Wire, ..rect() };
        assert!(wire.hit([50.0, 3.0], 2.0));
        assert!(!wire.hit([50.0, 20.0], 2.0));
        assert!(rect().hit([60.0, 30.0], 0.0));
    }

    #[test]
    fn snapping_keeps_length_and_rounds_the_angle() {
        let p = snap45([0.0, 0.0], [10.0, 1.0]);
        assert!((p[0] - 10.05).abs() < 0.01 && p[1].abs() < 1e-4, "{p:?}");
    }
}
