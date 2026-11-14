//! Real-world sizes: the image's scale (set by calibrating a measure against a known length),
//! the unit lengths are shown in, and the labels measures and sized boxes carry.

use crate::font;
use crate::model::{Annotation, Kind, Shape};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Micron,
    Millimetre,
    Centimetre,
    Metre,
    Inch,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::Micron, Unit::Millimetre, Unit::Centimetre, Unit::Metre, Unit::Inch];

    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Micron => "µm",
            Unit::Millimetre => "mm",
            Unit::Centimetre => "cm",
            Unit::Metre => "m",
            Unit::Inch => "in",
        }
    }

    pub fn in_mm(self) -> f32 {
        match self {
            Unit::Micron => 0.001,
            Unit::Millimetre => 1.0,
            Unit::Centimetre => 10.0,
            Unit::Metre => 1000.0,
            Unit::Inch => 25.4,
        }
    }
}

/// How image pixels map to real lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    /// Millimetres per image pixel, once calibrated.
    pub mm_per_px: Option<f32>,
    pub unit: Unit,
}

impl Default for Scale {
    fn default() -> Scale {
        Scale { mm_per_px: None, unit: Unit::Millimetre }
    }
}

/// A number with at most three significant decimals and no trailing zeros.
fn number(v: f32) -> String {
    let digits = if v.abs() >= 100.0 {
        0
    } else if v.abs() >= 10.0 {
        1
    } else {
        2
    };
    let s = format!("{v:.digits$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

impl Scale {
    /// `px` image pixels as text: in the chosen unit once calibrated, else in pixels.
    pub fn format(&self, px: f32) -> String {
        match self.mm_per_px {
            Some(k) => format!("{} {}", number(px * k / self.unit.in_mm()), self.unit.symbol()),
            None => format!("{} px", px.round()),
        }
    }

    /// The scale that makes a line `px` pixels long measure `value` in `unit`.
    pub fn calibrated(px: f32, value: f32, unit: Unit) -> Option<Scale> {
        (px > 0.5 && value > 0.0 && value.is_finite()).then(|| Scale { mm_per_px: Some(value * unit.in_mm() / px), unit })
    }
}

/// A size label: its text and where its box's centre sits, in image pixels.
pub struct Label {
    pub text: String,
    pub center: [f32; 2],
}

/// The label an annotation shows for its real size, if it shows one.
pub fn label(a: &Annotation, scale: &Scale) -> Option<Label> {
    match (&a.shape, a.kind) {
        (Shape::Arrow { from, to }, Kind::Measure) => {
            let len = (to[0] - from[0]).hypot(to[1] - from[1]);
            Some(Label { text: scale.format(len), center: [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0] })
        }
        (Shape::Rect { x, y, w, h }, _) if a.style.show_size => {
            let text = format!("{} × {}", scale.format(*w), scale.format(*h));
            let (_, th) = font::text_size(&text, &a.style);
            let gap = a.style.stroke_width / 2.0 + a.style.padding;
            Some(Label { text, center: [x + w / 2.0, y + h + gap + th / 2.0 + a.style.padding] })
        }
        _ => None,
    }
}

/// The two short ticks across each end of a measure line, each as a segment.
pub fn ticks(from: [f32; 2], to: [f32; 2], width: f32) -> [([f32; 2], [f32; 2]); 2] {
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = dx.hypot(dy).max(f32::EPSILON);
    let half = (width * 3.0).max(8.0);
    let n = [-dy / len * half, dx / len * half];
    let tick = |p: [f32; 2]| ([p[0] - n[0], p[1] - n[1]], [p[0] + n[0], p[1] + n[1]]);
    [tick(from), tick(to)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_converts_pixels_to_the_unit() {
        let s = Scale::calibrated(200.0, 4.0, Unit::Millimetre).unwrap();
        assert_eq!(s.format(100.0), "2 mm");
        let um = Scale { unit: Unit::Micron, ..s };
        assert_eq!(um.format(100.0), "2000 µm");
        assert_eq!(Scale::default().format(12.4), "12 px");
    }
}
