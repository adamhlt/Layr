//! Diagonal resize cursors. Rune has the system's row and column resize cursors but no
//! diagonal ones, so these are drawn once as 2x PNGs and handed to `Cursor::Image`.

use std::sync::OnceLock;

use rune::{Cursor, CursorImage};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// 24 × 24 points, drawn at 2x.
const PX: u32 = 48;

/// A double-headed arrow from top-left to bottom-right (`down`) or bottom-left to top-right.
fn draw(down: bool) -> &'static [u8] {
    let mut pm = Pixmap::new(PX, PX).expect("cursor pixmap");
    let c = PX as f32 / 2.0;
    // The arrow along +x, then turned onto the diagonal.
    let mut pb = PathBuilder::new();
    let (len, head, shaft) = (15.0, 7.0, 2.2);
    pb.move_to(-len, 0.0);
    pb.line_to(-len + head, -head);
    pb.line_to(-len + head, -shaft);
    pb.line_to(len - head, -shaft);
    pb.line_to(len - head, -head);
    pb.line_to(len, 0.0);
    pb.line_to(len - head, head);
    pb.line_to(len - head, shaft);
    pb.line_to(-len + head, shaft);
    pb.line_to(-len + head, head);
    pb.close();
    let path = pb.finish().expect("cursor path");
    let angle = if down { 45.0 } else { -45.0 };
    let ts = Transform::from_rotate(angle).post_translate(c, c);
    let mut white = Paint::default();
    white.set_color(Color::WHITE);
    white.anti_alias = true;
    let outline = Stroke { width: 3.0, line_join: LineJoin::Round, line_cap: LineCap::Round, ..Stroke::default() };
    pm.stroke_path(&path, &white, &outline, ts, None);
    let mut black = Paint::default();
    black.set_color(Color::BLACK);
    black.anti_alias = true;
    pm.fill_path(&path, &black, FillRule::Winding, ts, None);

    let mut rgba = Vec::with_capacity((PX * PX * 4) as usize);
    for p in pm.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let mut png = Vec::new();
    image::RgbaImage::from_raw(PX, PX, rgba)
        .expect("cursor pixels")
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("cursor png");
    Box::leak(png.into_boxed_slice())
}

/// The `↘↖` resize cursor (top-left and bottom-right corners).
pub fn resize_down() -> Cursor {
    static PNG: OnceLock<&'static [u8]> = OnceLock::new();
    Cursor::Image(CursorImage::png(PNG.get_or_init(|| draw(true)), [12.0, 12.0]))
}

/// The `↗↙` resize cursor (top-right and bottom-left corners).
pub fn resize_up() -> Cursor {
    static PNG: OnceLock<&'static [u8]> = OnceLock::new();
    Cursor::Image(CursorImage::png(PNG.get_or_init(|| draw(false)), [12.0, 12.0]))
}
