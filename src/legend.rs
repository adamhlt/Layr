//! Legends: a framed list with one row per look used on the image, each a small sample of
//! that look and a description you can edit ("magenta = 0.1 mm enamelled wire").
//!
//! Samples are ordinary annotations laid out inside the frame, so the canvas, PNG and PDF all
//! draw them with the code that draws the annotations themselves.

use crate::font;
use crate::model::{Annotation, Kind, Rgba, Shape, Style};

/// One row: the look it shows and what it means.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendItem {
    pub kind: Kind,
    pub style: Style,
    pub label: String,
}

/// What makes two annotations the same row: their kind and colours.
fn key(kind: Kind, st: &Style) -> (Kind, [u8; 4], [u8; 4], Option<[u8; 4]>) {
    (kind, st.stroke.0, st.fill.0, st.hatch.as_ref().map(|h| h.color.0))
}

fn default_label(kind: Kind, st: &Style) -> String {
    let what = match kind {
        Kind::Rect => "area",
        Kind::Hatch => "hatched area",
        Kind::Badge => "badge",
        Kind::Wire => "wire",
        Kind::Arrow => "arrow",
        Kind::Dot => "point",
        Kind::Measure => "measure",
        Kind::Text | Kind::Legend => "note",
    };
    let main = if matches!(kind, Kind::Badge | Kind::Dot) { st.fill } else { st.stroke };
    format!("{} {what}", colour_name(main))
}

/// A plain name for the nearest of a few colours.
fn colour_name(c: Rgba) -> &'static str {
    let names: [(&str, [u8; 3]); 10] = [
        ("Green", [46, 204, 64]),
        ("Red", [235, 45, 45]),
        ("Magenta", [240, 40, 230]),
        ("Yellow", [255, 220, 0]),
        ("Cyan", [40, 190, 240]),
        ("Orange", [255, 140, 20]),
        ("White", [255, 255, 255]),
        ("Black", [0, 0, 0]),
        ("Blue", [40, 80, 230]),
        ("Purple", [140, 60, 200]),
    ];
    let d = |n: [u8; 3]| (0..3).map(|i| (c.0[i] as i32 - n[i] as i32).pow(2)).sum::<i32>();
    names.iter().min_by_key(|(_, n)| d(*n)).map_or("", |(name, _)| name)
}

/// One row per distinct look among `anns`, in the order they first appear. Captions and
/// legends describe themselves and get no row. Rows of `keep` with the same look keep their
/// descriptions.
pub fn items_from(anns: &[Annotation], keep: &[LegendItem]) -> Vec<LegendItem> {
    let mut out: Vec<LegendItem> = Vec::new();
    for a in anns.iter().filter(|a| !matches!(a.kind, Kind::Text | Kind::Legend)) {
        let k = key(a.kind, &a.style);
        if out.iter().any(|i| key(i.kind, &i.style) == k) {
            continue;
        }
        let label = keep.iter().find(|i| key(i.kind, &i.style) == k).map_or_else(|| default_label(a.kind, &a.style), |i| i.label.clone());
        out.push(LegendItem { kind: a.kind, style: a.style.clone(), label });
    }
    out
}

pub struct Row {
    pub sample: Annotation,
    pub label: String,
    pub label_origin: [f32; 2],
}

pub struct Layout {
    pub frame: [f32; 4],
    /// The title's top-left, when there is one.
    pub title: Option<[f32; 2]>,
    pub rows: Vec<Row>,
}

/// A sample of `item` filling `[x, y, w, h]`, its lengths shrunk to fit the row.
fn sample(item: &LegendItem, [x, y, w, h]: [f32; 4]) -> Annotation {
    let mut st = item.style.clone();
    st.stroke_width = st.stroke_width.min(h * 0.22);
    st.radius = st.radius.min(h * 0.25);
    st.show_size = false;
    if let Some(hs) = &mut st.hatch {
        hs.spacing = hs.spacing.min(h * 0.32);
        hs.width = hs.width.min(h * 0.12);
    }
    let cy = y + h / 2.0;
    let (kind, shape) = match item.kind {
        Kind::Rect | Kind::Hatch | Kind::Text | Kind::Legend => (Kind::Rect, Shape::Rect { x, y, w, h }),
        Kind::Badge => (Kind::Badge, Shape::Rect { x: x + (w - h) / 2.0, y, w: h, h }),
        Kind::Wire => (Kind::Wire, Shape::Polyline { points: vec![[x, cy], [x + w, cy]] }),
        Kind::Arrow => (Kind::Arrow, Shape::Arrow { from: [x, cy], to: [x + w, cy] }),
        Kind::Measure => (Kind::Measure, Shape::Arrow { from: [x + st.stroke_width, cy], to: [x + w - st.stroke_width, cy] }),
        Kind::Dot => (Kind::Dot, Shape::Dot { center: [x + w / 2.0, cy], radius: h * 0.32 }),
    };
    Annotation { id: 0, kind, shape, style: st, text: String::new(), legend: Vec::new() }
}

/// Where a legend's frame, title and rows go. `a.text` is the title; `a.legend` the rows.
pub fn layout(a: &Annotation) -> Layout {
    let st = &a.style;
    let pos = match a.shape {
        Shape::Text { pos } => pos,
        _ => [0.0, 0.0],
    };
    let pad = st.padding;
    let line = st.font_size * font::LINE_HEIGHT;
    let (sw, sh) = (st.font_size * 1.8, st.font_size * 1.0);
    let gap = st.font_size * 0.6;
    let row_h = line.max(sh);
    let mut y = pos[1] + pad;
    let mut width: f32 = 0.0;
    let title = (!a.text.trim().is_empty()).then(|| {
        let (tw, th) = font::text_size(&a.text, st);
        width = width.max(tw);
        let at = [pos[0] + pad, y];
        y += th + gap * 0.6;
        at
    });
    let mut rows = Vec::new();
    for item in &a.legend {
        let (lw, lh) = font::text_size(if item.label.is_empty() { " " } else { &item.label }, st);
        width = width.max(sw + gap + lw);
        rows.push(Row {
            sample: sample(item, [pos[0] + pad, y + (row_h - sh) / 2.0, sw, sh]),
            label: item.label.clone(),
            label_origin: [pos[0] + pad + sw + gap, y + (row_h - lh) / 2.0],
        });
        y += row_h + gap * 0.4;
    }
    if !rows.is_empty() {
        y -= gap * 0.4;
    }
    Layout { frame: [pos[0], pos[1], width + 2.0 * pad, y + pad - pos[1]], title, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(kind: Kind) -> Annotation {
        Annotation {
            id: 1,
            kind,
            shape: Shape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 },
            style: kind.default_style(),
            text: String::new(),
            legend: Vec::new(),
        }
    }

    #[test]
    fn one_row_per_look_and_descriptions_survive_an_update() {
        let anns = vec![ann(Kind::Wire), ann(Kind::Wire), ann(Kind::Hatch), ann(Kind::Text)];
        let mut items = items_from(&anns, &[]);
        assert_eq!(items.len(), 2, "two wires share a row; captions get none");
        assert_eq!(items[0].label, "Magenta wire");
        items[0].label = "0.1 mm enamelled wire".into();
        let again = items_from(&anns, &items);
        assert_eq!(again[0].label, "0.1 mm enamelled wire");
    }
}
