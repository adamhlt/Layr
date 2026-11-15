//! The window: tool rail, canvas, properties panel, colour editor and status bar.

use std::f32::consts::FRAC_PI_2;

use rune::file_dialog::FileDialogOptions;
use rune::tok::{self, TextStyle, type_};
use rune::{
    Color, Cursor, Cx, Drag, El, Key, KeyEvent, LineCap, MagnifyEvent, Phase, PointerButton, ScrollSpec, Shape, Tx, WheelEvent, canvas,
    col, el, row, spacer, text, text_input,
};

use crate::color::{self, ColorTool, Target};
use crate::editor::{Editor, Hover, PanelTab, Tool};
use crate::measure::Unit;
use crate::model::{Annotation, BadgeShape, Hatch, Kind, PALETTE, Rgba, Shape as Geo, Style};
use crate::render::{ExportOptions, Format};
use crate::stitch::job::{self as stitching, Mode, Output, Stage};
use crate::update::{self, State};
use crate::{cursors, font, vector};

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];
const PANEL_W: f32 = 290.0;
const RAIL_W: f32 = 168.0;
/// The colour editor's inner width.
const PICKER_W: f32 = 300.0;
/// The stitching dialog's inner width.
const STITCH_W: f32 = 360.0;

fn theme() -> tok::Theme {
    tok::theme(true)
}

// ---- Small controls ---------------------------------------------------------------------------

fn button(key: impl Into<String>, label: &str, active: bool, enabled: bool, accent: Color) -> El {
    let t = theme();
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .px(10.0)
        .items_center()
        .radius(6.0)
        .fill(if active { accent } else { t.control_track })
        .child(text(label).font(type_::LABEL).color(if enabled { t.text } else { t.text_secondary.alpha(0.5) }));
    // `disabled` only tells assistive tech; `inert` is what stops the press, which would
    // otherwise set state that takes effect once the action becomes available.
    if enabled { b.focusable(true).hover(t.hover).cursor(Cursor::Pointer) } else { b.disabled(true).inert() }
}

fn section(title: &str) -> El {
    text(title).font(type_::SECTION).color(theme().text_secondary)
}

fn swatch(key: String, c: Rgba, on: bool) -> El {
    let t = theme();
    el().key(key)
        .size(22.0, 22.0)
        .radius(5.0)
        .fill(c.with_alpha(255).to_rune())
        .border(if on { 2.0 } else { 1.0 }, if on { Color::WHITE } else { t.card_border })
        .cursor(Cursor::Pointer)
}

/// The built-in colours, the user's own, and a button opening the colour editor on `target`.
fn swatches(key: &'static str, current: Rgba, none: bool, custom: &[Rgba], target: Target) -> El {
    let t = theme();
    let mut items = Vec::new();
    if none {
        let on = current.0[3] == 0;
        items.push(
            el().key(format!("{key}-none"))
                .size(22.0, 22.0)
                .radius(5.0)
                .fill(t.card)
                .border(if on { 2.0 } else { 1.0 }, if on { Color::WHITE } else { t.card_border })
                .child(text("∅").font(type_::LABEL).color(t.text_secondary))
                .center()
                .cursor(Cursor::Pointer)
                .on_press(move |s: &mut Editor| s.edit_style(|st| target.set(st, current.with_alpha(0)))),
        );
    }
    for (i, c) in PALETTE.iter().chain(custom).copied().enumerate() {
        let on = current.0[3] > 0 && current.same_hue(c);
        items.push(swatch(format!("{key}-{i}"), c, on).on_press(move |s: &mut Editor| {
            // Keep the current opacity when only the hue changes.
            let a = if current.0[3] == 0 { 255 } else { current.0[3] };
            s.edit_style(|st| target.set(st, c.with_alpha(a)))
        }));
    }
    items.push(
        el().key(format!("{key}-more"))
            .size(22.0, 22.0)
            .radius(5.0)
            .fill(t.control_track)
            .hover(t.hover)
            .center()
            .cursor(Cursor::Pointer)
            .child(text("+").font(type_::TITLE).color(t.text))
            .on_press(move |s: &mut Editor| s.open_color_tool(target)),
    );
    grid(items, 9)
}

/// Lays `items` out `per_row` to a row. (`wrap` rows overflow inside a scroll view.)
fn grid(items: Vec<El>, per_row: usize) -> El {
    let mut rows = col().w_full().gap(5.0);
    let mut it = items.into_iter().peekable();
    while it.peek().is_some() {
        rows = rows.child(row().gap(5.0).children(it.by_ref().take(per_row)));
    }
    rows
}

/// The panel's inner width, which sliders span.
const SLIDER_W: f32 = PANEL_W - 28.0;

/// A slider over `min..=max` for one style value. A drag is one undo step.
#[allow(clippy::too_many_arguments)]
fn slider(key: &'static str, label: &str, value: f32, min: f32, max: f32, shown: String, accent: Color, set: fn(&mut Style, f32)) -> El {
    let t = theme();
    let frac = ((value - min) / (max - min).max(f32::EPSILON)).clamp(0.0, 1.0);
    col()
        .w_full()
        .gap(2.0)
        .child(
            row()
                .w_full()
                .items_center()
                .child(text(label).font(type_::BODY).color(t.text))
                .child(spacer())
                .child(text(shown).key(format!("{key}-value")).font(type_::LABEL).color(t.text_secondary)),
        )
        .child(
            el().key(key)
                .role("slider")
                .label(label)
                .w(SLIDER_W)
                .h(18.0)
                .cursor(Cursor::Pointer)
                .child(el().absolute(0.0, 7.0).size(SLIDER_W, 4.0).radius(2.0).fill(t.control_track).inert())
                .child(el().absolute(0.0, 7.0).size(frac * SLIDER_W, 4.0).radius(2.0).fill(accent).inert())
                .child(
                    el().absolute(frac * (SLIDER_W - 14.0), 2.0)
                        .size(14.0, 14.0)
                        .radius(7.0)
                        .fill(Color::WHITE)
                        .border(1.0, Color::BLACK.alpha(0.35))
                        .inert(),
                )
                .on_drag(move |s: &mut Editor, d: &mut Drag, _| {
                    let v = min + (max - min) * ((d.x - 7.0) / (d.w - 14.0).max(1.0)).clamp(0.0, 1.0);
                    s.slide_style(d.phase == Phase::Down, |st| set(st, v));
                }),
        )
}

fn checkbox(key: &'static str, label: &str, on: bool, toggle: fn(&mut Style)) -> El {
    check(key, label, on).on_press(move |s: &mut Editor| s.edit_style(toggle))
}

/// A checkbox's look; the caller adds what pressing it does.
fn check(key: &'static str, label: &str, on: bool) -> El {
    let t = theme();
    row()
        .key(key)
        .gap(8.0)
        .items_center()
        .cursor(Cursor::Pointer)
        .role("checkbox")
        .label(label)
        .checked(on)
        .focusable(true)
        .child(
            el().size(18.0, 18.0)
                .radius(4.0)
                .fill(if on { Color::WHITE } else { t.control_track })
                .border(1.0, t.card_border)
                .center()
                .child(text(if on { "✓" } else { "" }).font(type_::LABEL).color(Color::BLACK)),
        )
        .child(text(label).font(type_::BODY).color(t.text))
}

/// Two equal buttons a row: wrapping rows overflow a scroll view's width.
fn pair(a: El, b: El) -> El {
    row().w_full().gap(6.0).child(a.grow(1.0).center()).child(b.grow(1.0).center())
}

fn px(v: f32) -> String {
    format!("{} px", (v * 10.0).round() / 10.0)
}
fn pct(a: u8) -> String {
    format!("{} %", (a as f32 / 2.55).round())
}
/// An opacity slider's 0..=100 back to an alpha byte.
fn alpha(v: f32) -> u8 {
    (v * 2.55).round().clamp(0.0, 255.0) as u8
}
fn percent(a: u8) -> f32 {
    a as f32 / 2.55
}

// ---- Rail -------------------------------------------------------------------------------------

/// The open menu's single-image choice: picks the file, opens it and closes the menu.
fn choose_image(_: &mut Editor, tx: &mut Tx) {
    let options = FileDialogOptions { title: "Open an image".into(), ..Default::default() }.filter("Images", &IMAGE_EXTENSIONS);
    tx.open_file(options, |s: &mut Editor, result, _| match result {
        Ok(Some(path)) => {
            s.stitcher.close();
            s.load(&path);
        }
        Ok(None) => {}
        Err(e) => s.status = format!("Open dialog unavailable: {e}"),
    });
}

/// Adds every image in a folder to the list of photos to stitch.
fn add_folder(_: &mut Editor, tx: &mut Tx) {
    let options = FileDialogOptions { title: "Add the images of a folder".into(), ..Default::default() };
    tx.open_folder(options, |s: &mut Editor, result, _| match result {
        Ok(Some(dir)) => {
            let images = stitching::images_in(&dir);
            if images.is_empty() {
                s.status = format!("No images in {}", dir.display());
            }
            s.stitcher.add(images);
        }
        Ok(None) => {}
        Err(e) => s.status = format!("Folder dialog unavailable: {e}"),
    });
}

fn export_dialog(s: &mut Editor, tx: &mut Tx) {
    if s.picture.is_none() {
        return;
    }
    let format = s.export.format;
    let options =
        FileDialogOptions { title: format!("Export as {}", format.name()), file_name: Some(s.export_name()), ..Default::default() }
            .filter(format.name(), format.extensions());
    s.export_open = false;
    tx.save_file(options, |s: &mut Editor, result, _| match result {
        Ok(Some(path)) => s.export(&path),
        Ok(None) => {}
        Err(e) => s.status = format!("Save dialog unavailable: {e}"),
    });
}

/// A small drawing of what each tool makes, 18 points square.
fn tool_icon(tool: Tool, c: Color) -> El {
    let k = |x: f32, y: f32| [x, y];
    let shape = match tool {
        Tool::Select => {
            Shape::new().fill(vec![k(4.0, 2.0), k(14.0, 11.0), k(9.5, 11.5), k(12.0, 16.5), k(10.0, 17.5), k(7.5, 12.5), k(4.0, 15.0)], c)
        }
        Tool::Draw(Kind::Rect) => {
            Shape::new().stroke_with(vec![k(2.0, 4.0), k(16.0, 4.0), k(16.0, 14.0), k(2.0, 14.0)], true, 1.6, LineCap::Round, c)
        }
        Tool::Draw(Kind::Hatch) => {
            let mut sh =
                Shape::new().stroke_with(vec![k(2.0, 3.0), k(16.0, 3.0), k(16.0, 15.0), k(2.0, 15.0)], true, 1.4, LineCap::Round, c);
            for (a, b) in
                [(k(2.0, 9.0), k(8.0, 3.0)), (k(2.0, 15.0), k(14.0, 3.0)), (k(8.0, 15.0), k(16.0, 7.0)), (k(14.0, 15.0), k(16.0, 13.0))]
            {
                sh = sh.line(a, b, 1.0, LineCap::Butt, c);
            }
            sh
        }
        Tool::Draw(Kind::Badge) => {
            let ring: Vec<[f32; 2]> = (0..24)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 24.0;
                    k(9.0 + 7.5 * a.cos(), 9.0 + 7.5 * a.sin())
                })
                .collect();
            Shape::new().stroke_with(ring, true, 1.6, LineCap::Round, c).line(k(9.5, 5.0), k(9.5, 13.0), 1.6, LineCap::Round, c).line(
                k(9.5, 5.0),
                k(7.5, 7.0),
                1.6,
                LineCap::Round,
                c,
            )
        }
        Tool::Draw(Kind::Wire) => Shape::new().stroke(vec![k(2.0, 3.0), k(2.0, 9.0), k(9.0, 13.0), k(16.0, 13.0), k(16.0, 16.0)], 1.8, c),
        Tool::Draw(Kind::Arrow) => {
            Shape::new().line(k(3.0, 15.0), k(13.0, 5.0), 1.8, LineCap::Round, c).fill(vec![k(16.0, 2.0), k(14.5, 9.0), k(9.0, 3.5)], c)
        }
        Tool::Draw(Kind::Dot) => Shape::new().fill(
            (0..16)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 16.0;
                    k(9.0 + 5.0 * a.cos(), 9.0 + 5.0 * a.sin())
                })
                .collect::<Vec<_>>(),
            c,
        ),
        Tool::Draw(Kind::Text) => {
            Shape::new().line(k(3.0, 3.0), k(15.0, 3.0), 2.0, LineCap::Round, c).line(k(9.0, 3.0), k(9.0, 16.0), 2.0, LineCap::Round, c)
        }
        // Legends come from the rail's Legend button, not a drawing tool.
        Tool::Draw(Kind::Legend) => Shape::new(),
        Tool::Draw(Kind::Measure) => Shape::new()
            .line(k(2.0, 12.0), k(16.0, 12.0), 1.6, LineCap::Butt, c)
            .line(k(2.0, 8.0), k(2.0, 16.0), 1.6, LineCap::Butt, c)
            .line(k(16.0, 8.0), k(16.0, 16.0), 1.6, LineCap::Butt, c)
            .line(k(6.5, 10.0), k(6.5, 12.0), 1.2, LineCap::Butt, c)
            .line(k(11.5, 10.0), k(11.5, 12.0), 1.2, LineCap::Butt, c)
            .line(k(9.0, 9.0), k(9.0, 12.0), 1.2, LineCap::Butt, c),
    };
    canvas(shape).size(18.0, 18.0)
}

fn legend_icon(c: Color) -> El {
    let k = |x: f32, y: f32| [x, y];
    canvas(
        Shape::new()
            .stroke_with(vec![k(1.5, 2.0), k(16.5, 2.0), k(16.5, 16.0), k(1.5, 16.0)], true, 1.4, LineCap::Round, c)
            .line(k(4.0, 6.5), k(7.0, 6.5), 2.4, LineCap::Butt, c)
            .line(k(9.0, 6.5), k(14.0, 6.5), 1.2, LineCap::Round, c)
            .line(k(4.0, 11.5), k(7.0, 11.5), 2.4, LineCap::Butt, c)
            .line(k(9.0, 11.5), k(14.0, 11.5), 1.2, LineCap::Round, c),
    )
    .size(18.0, 18.0)
}

fn rail_button(key: impl Into<String>, icon: Option<El>, label: &str, hint: &str, active: bool, enabled: bool, accent: Color) -> El {
    let t = theme();
    let ink = if !enabled {
        t.text_secondary.alpha(0.5)
    } else if active {
        Color::WHITE
    } else {
        t.text
    };
    let mut b = row().key(key).role("button").label(label).w_full().h(32.0).px(8.0).gap(8.0).items_center().radius(6.0).fill(if active {
        accent
    } else {
        Color::TRANSPARENT
    });
    if let Some(icon) = icon {
        b = b.child(icon);
    }
    b = b.child(text(label).font(type_::LABEL).color(ink)).child(spacer());
    if !hint.is_empty() {
        b = b.child(text(hint).font(type_::LABEL).color(if active { Color::WHITE.alpha(0.7) } else { t.text_secondary }));
    }
    // `disabled` only tells assistive tech; `inert` is what stops the press, which would
    // otherwise set state that takes effect once the action becomes available.
    if enabled { b.focusable(true).hover(t.hover).cursor(Cursor::Pointer) } else { b.disabled(true).inert() }
}

fn rail(s: &Editor, accent: Color) -> El {
    let t = theme();
    let has = s.picture.is_some();
    let sep = || el().w_full().h(1.0).fill(t.separator).my(6.0);
    let mut tools = col().w_full().gap(2.0);
    for (tool, name, key) in Tool::ALL {
        let on = s.tool == tool;
        let ink = if !has {
            t.text_secondary.alpha(0.5)
        } else if on {
            Color::WHITE
        } else {
            t.text
        };
        tools = tools.child(
            rail_button(format!("tool-{key}"), Some(tool_icon(tool, ink)), name, key, on, has, accent)
                .on_press(move |s: &mut Editor| s.set_tool(tool)),
        );
    }
    let zoom = row()
        .w_full()
        .gap(4.0)
        .items_center()
        .child(button("zoom-out", "−", false, has, accent).on_press(|s: &mut Editor| s.zoom_center(1.0 / 1.25)))
        .child(text(format!("{:.0} %", s.zoom * 100.0)).key("zoom").font(type_::LABEL).color(t.text_secondary).grow(1.0).center())
        .child(button("zoom-in", "+", false, has, accent).on_press(|s: &mut Editor| s.zoom_center(1.25)));
    col()
        .key("rail")
        .w(RAIL_W)
        .h_full()
        .min_h(0.0)
        .padding(8.0)
        .gap(2.0)
        .fill(t.card)
        .child(rail_button("open", None, "Open…", "Ctrl+O", false, true, accent).on_press(|s: &mut Editor| s.stitcher.open()))
        .child(rail_button("export", None, "Export…", "Ctrl+E", false, has, accent).on_press(|s: &mut Editor| s.export_open = true))
        .child(sep())
        .child(tools)
        .child(
            rail_button(
                "add-legend",
                Some(legend_icon(if has { t.text } else { t.text_secondary.alpha(0.5) })),
                "Legend",
                "G",
                false,
                has,
                accent,
            )
            .on_press(|s: &mut Editor| s.add_legend()),
        )
        .child(sep())
        .child(rail_button("undo", None, "Undo", "Ctrl+Z", false, s.can_undo(), accent).on_press(|s: &mut Editor| s.undo()))
        .child(rail_button("redo", None, "Redo", "Ctrl+Y", false, s.can_redo(), accent).on_press(|s: &mut Editor| s.redo()))
        .child(spacer())
        .child(zoom)
        .child(rail_button("fit", None, "Fit", "Ctrl+F", false, has, accent).on_press(|s: &mut Editor| s.fit()))
}

// ---- Canvas -----------------------------------------------------------------------------------

/// The selection's outline and handles, in viewport points.
fn selection_overlay(s: &Editor, accent: Color) -> Vec<El> {
    let Some(a) = s.selection() else { return Vec::new() };
    let [_, _, vw, vh] = s.viewport;
    let mut out = Vec::new();
    if matches!(a.shape, Geo::Rect { .. } | Geo::Text { .. } | Geo::Dot { .. }) {
        let [x, y, w, h] = a.bounds();
        let p0 = s.to_screen([x, y]);
        let p1 = s.to_screen([x + w, y + h]);
        let (l, t, r, b) = (p0[0] - 3.0, p0[1] - 3.0, p1[0] + 3.0, p1[1] + 3.0);
        let outline = Shape::new().stroke_with(vec![[l, t], [r, t], [r, b], [l, b]], true, 1.5, LineCap::Butt, accent);
        out.push(canvas(outline).key("sel-outline").absolute(0.0, 0.0).size(vw, vh).inert());
    }
    for (i, h) in a.handles().into_iter().enumerate() {
        let q = s.to_screen(h);
        out.push(
            el().key(format!("handle-{i}"))
                .absolute(q[0] - 5.0, q[1] - 5.0)
                .size(10.0, 10.0)
                .radius(2.0)
                .fill(Color::WHITE)
                .border(1.5, accent)
                .inert(),
        );
    }
    out
}

fn canvas_cursor(s: &Editor) -> Cursor {
    if s.panning() {
        return Cursor::Grabbing;
    }
    if s.space {
        return Cursor::Grab;
    }
    if s.moving() {
        return Cursor::Move;
    }
    match s.hover {
        Hover::Body | Hover::Point => Cursor::Move,
        Hover::ResizeX => Cursor::ResizeColumn,
        Hover::ResizeY => Cursor::ResizeRow,
        Hover::ResizeDown => cursors::resize_down(),
        Hover::ResizeUp => cursors::resize_up(),
        Hover::Nothing => match s.tool {
            Tool::Select => Cursor::Default,
            Tool::Draw(_) => Cursor::Crosshair,
        },
    }
}

fn viewport(s: &Editor, accent: Color) -> El {
    let t = theme();
    let mut v = el()
        .key("viewport")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .clip_soft(0.0)
        .fill(rune::color::hex("#141417").unwrap_or(Color::BLACK))
        .cursor(canvas_cursor(s))
        .on_layout(|s: &mut Editor, r| s.set_viewport(r))
        .pointer_buttons([PointerButton::Middle])
        .on_drag(|s: &mut Editor, d, tx| s.drag(d, tx))
        .on_pointer_move_local(|s: &mut Editor, e, tx| s.pointer_moved([e.x, e.y], tx.mods().shift, crate::mouse::right_button_down()))
        .on_hover_change(|s: &mut Editor, inside, _| {
            if !inside {
                s.pointer_left();
            }
        })
        .on_wheel(|s: &mut Editor, e: WheelEvent, _| {
            if s.picture.is_none() {
                return false;
            }
            s.zoom_about([e.x, e.y], (e.dy * 0.002).exp());
            true
        })
        .on_magnify(|s: &mut Editor, e: MagnifyEvent, _| {
            s.zoom_about([e.x, e.y], 1.0 + e.delta);
            true
        });

    match &s.picture {
        Some(pic) => {
            let (iw, ih) = pic.size();
            let world = el()
                .key("world")
                .absolute(0.0, 0.0)
                .size(iw, ih)
                .origin(0.0, 0.0)
                .live_offset(&s.live_pan)
                .live_scale(&s.live_zoom)
                .pixels(&s.base)
                .children(s.anns.iter().flat_map(|a| vector::annotation(a, iw, ih, &s.scale)))
                .inert();
            v = v.child(world).children(selection_overlay(s, accent));
        }
        None => {
            v = v.center().child(
                col()
                    .gap(14.0)
                    .items_center()
                    .child(text("No image").font(TextStyle::new(20.0, 600.0)).color(t.text))
                    .child(text("Drop an image here, or open one (PNG, JPEG, WebP, BMP).").font(type_::BODY).color(t.text_secondary))
                    .child(button("open-empty", "Open an image…", true, true, accent).on_press(|s: &mut Editor| s.stitcher.open())),
            );
        }
    }
    v
}

// ---- Properties panel -------------------------------------------------------------------------

fn hatch_controls(st: &Style, custom: &[Rgba], unit: f32, accent: Color) -> El {
    let on = st.hatch.is_some();
    let mut c = col().w_full().gap(10.0).child(checkbox("hatch-toggle", "Hatching", on, |st| {
        st.hatch = match st.hatch.take() {
            Some(_) => None,
            None => Some(Hatch {
                color: st.stroke.with_alpha(200),
                spacing: (st.stroke_width * 4.0).max(8.0),
                width: (st.stroke_width * 0.8).max(1.0),
                angle_deg: 45.0,
            }),
        }
    }));
    let Some(h) = &st.hatch else { return c };
    fn hatch(st: &mut Style, f: impl FnOnce(&mut Hatch)) {
        if let Some(h) = &mut st.hatch {
            f(h)
        }
    }
    c = c
        .child(swatches("hatch-color", h.color, false, custom, Target::Hatch))
        .child(slider("hatch-alpha", "Opacity", percent(h.color.0[3]), 5.0, 100.0, pct(h.color.0[3]), accent, |st, v| {
            hatch(st, |h| h.color = h.color.with_alpha(alpha(v)))
        }))
        .child(slider("hatch-spacing", "Spacing", h.spacing, 2.0, 80.0 * unit, px(h.spacing), accent, |st, v| hatch(st, |h| h.spacing = v)))
        .child(slider("hatch-width", "Line width", h.width, 0.5, 20.0 * unit, px(h.width), accent, |st, v| hatch(st, |h| h.width = v)))
        .child(slider("hatch-angle", "Angle", h.angle_deg, 0.0, 180.0, format!("{}°", h.angle_deg.round()), accent, |st, v| {
            // Snap to 5° steps.
            hatch(st, |h| h.angle_deg = (v / 5.0).round() * 5.0)
        }));
    c
}

fn badge_shapes(current: BadgeShape, accent: Color) -> El {
    let t = theme();
    let mut r = row().w_full().gap(4.0);
    for (shape, name) in BadgeShape::ALL {
        let on = current == shape;
        let ink = if on { Color::WHITE } else { t.text };
        let b = [3.0, 3.0, 16.0, 16.0];
        let icon = match shape.polygon(b) {
            Some(poly) => Shape::new().stroke_with(poly, true, 1.5, LineCap::Round, ink),
            None => {
                let r = if shape == BadgeShape::Rounded { 4.0 } else { 0.0 };
                let poly: Vec<[f32; 2]> = if r > 0.0 {
                    [([15.0, 7.0], -90.0f32), ([15.0, 15.0], 0.0), ([7.0, 15.0], 90.0), ([7.0, 7.0], 180.0)]
                        .into_iter()
                        .flat_map(|(c, a0)| {
                            (0..=4).map(move |i| {
                                let a = (a0 + 22.5 * i as f32).to_radians();
                                [c[0] + r * a.cos(), c[1] + r * a.sin()]
                            })
                        })
                        .collect()
                } else {
                    vec![[3.0, 3.0], [19.0, 3.0], [19.0, 19.0], [3.0, 19.0]]
                };
                Shape::new().stroke_with(poly, true, 1.5, LineCap::Round, ink)
            }
        };
        r = r.child(
            el().key(format!("badge-{name}"))
                .role("radio")
                .label(name)
                .checked(on)
                .grow(1.0)
                .h(32.0)
                .radius(6.0)
                .center()
                .fill(if on { accent } else { t.control_track })
                .hover(t.hover)
                .cursor(Cursor::Pointer)
                .child(canvas(icon).size(22.0, 22.0))
                .on_press(move |s: &mut Editor| s.edit_style(|st| st.badge = shape)),
        );
    }
    r
}

fn font_name(st: &Style) -> &str {
    st.font.as_deref().unwrap_or("Inter (default)")
}

/// The font picker: a search field and the matching installed families, each in its own face.
fn font_menu(s: &Editor, current: &Style) -> El {
    let t = theme();
    let query = s.font_query.text.trim().to_lowercase();
    let row_for = |key: String, label: String, family: Option<String>, on: bool| {
        let style = match &family {
            Some(name) => font::text_style(&Style { font: Some(name.clone()), bold: false, ..current.clone() }, 14.0),
            None => TextStyle::new(14.0, 500.0),
        };
        row()
            .key(key)
            .w_full()
            .h(28.0)
            .px(8.0)
            .items_center()
            .radius(5.0)
            .fill(if on { t.control_track } else { Color::TRANSPARENT })
            .hover(t.hover)
            .cursor(Cursor::Pointer)
            .child(text(label).font(style).color(t.text).truncate())
            .on_press(move |s: &mut Editor| {
                let family = family.clone();
                s.font_menu = false;
                s.edit_style(|st| st.font = family);
            })
    };
    let mut list = col().w_full().gap(1.0);
    if query.is_empty() || "inter default".contains(&query) {
        list = list.child(row_for("font-default".into(), "Inter (default)".into(), None, current.font.is_none()));
    }
    let matches = font::families().iter().filter(|f| query.is_empty() || f.to_lowercase().contains(&query));
    let mut shown = 0;
    for (i, name) in matches.take(40).enumerate() {
        let on = current.font.as_deref() == Some(name.as_str());
        list = list.child(row_for(format!("font-{i}"), name.clone(), Some(name.clone()), on));
        shown += 1;
    }
    if shown == 0 && !query.is_empty() {
        list = list.child(text("No installed font matches.").font(type_::LABEL).color(t.text_secondary));
    }
    col()
        .key("font-menu")
        .w_full()
        .gap(6.0)
        .padding(8.0)
        .radius(8.0)
        .fill(t.bg)
        .border(1.0, t.card_border)
        .child(
            el().w_full().padding(6.0).radius(5.0).fill(t.card).child(
                text_input(&s.font_query, type_::BODY)
                    .key("font-search")
                    .label("Search fonts")
                    .w_full()
                    .color(t.text)
                    .placeholder("Search installed fonts…", t.text_secondary)
                    .on_edit(|s: &mut Editor| &mut s.font_query),
            ),
        )
        .child(col().key("font-list").w_full().h(220.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
}

fn tabs(current: PanelTab, has_text: bool, accent: Color) -> El {
    let t = theme();
    let mut r = row().key("tabs").w_full().gap(2.0).padding(2.0).radius(8.0).fill(t.bg);
    for (tab, name) in PanelTab::ALL {
        if tab == PanelTab::Text && !has_text {
            continue;
        }
        let on = tab == current;
        r = r.child(
            el().key(format!("tab-{name}"))
                .role("tab")
                .label(name)
                .checked(on)
                .grow(1.0)
                .h(28.0)
                .radius(6.0)
                .center()
                .fill(if on { accent } else { Color::TRANSPARENT })
                .hover(t.hover)
                .cursor(Cursor::Pointer)
                .child(text(name).font(type_::LABEL).color(if on { Color::WHITE } else { t.text_secondary }))
                .on_press(move |s: &mut Editor| s.panel_tab = tab),
        );
    }
    r
}

fn style_tab(s: &Editor, a: &Annotation, unit: f32, accent: Color) -> El {
    let st = &a.style;
    let custom = &s.palette[..];
    let boxy = matches!(a.kind, Kind::Rect | Kind::Hatch | Kind::Badge);
    let mut c = col().w_full().gap(10.0);
    if a.kind == Kind::Badge {
        c = c.child(section("Shape")).child(badge_shapes(st.badge, accent));
    }
    if a.kind == Kind::Measure {
        c = c.child(scale_section(s, a, accent));
    }
    if boxy {
        c = c.child(checkbox("show-size", "Show real size", st.show_size, |st| st.show_size = !st.show_size));
    }
    let stroke_title = if matches!(a.kind, Kind::Wire | Kind::Arrow | Kind::Measure) { "Line" } else { "Outline" };
    c = c
        .child(section(stroke_title))
        .child(swatches("stroke", st.stroke, false, custom, Target::Stroke))
        .child(slider("stroke-width", "Width", st.stroke_width, 0.0, 40.0 * unit, px(st.stroke_width), accent, |st, v| st.stroke_width = v))
        .child(slider("stroke-alpha", "Opacity", percent(st.stroke.0[3]), 5.0, 100.0, pct(st.stroke.0[3]), accent, |st, v| {
            st.stroke = st.stroke.with_alpha(alpha(v))
        }));
    if !matches!(a.kind, Kind::Wire | Kind::Arrow) {
        let title = match a.kind {
            Kind::Text => "Background",
            Kind::Measure => "Label background",
            _ => "Fill",
        };
        c = c.child(section(title)).child(swatches("fill", st.fill, true, custom, Target::Fill)).child(slider(
            "fill-alpha",
            "Opacity",
            percent(st.fill.0[3]),
            0.0,
            100.0,
            pct(st.fill.0[3]),
            accent,
            |st, v| st.fill = st.fill.with_alpha(alpha(v)),
        ));
    }
    let rounded = matches!(a.kind, Kind::Rect | Kind::Hatch | Kind::Text) || (a.kind == Kind::Badge && st.badge == BadgeShape::Rounded);
    if rounded {
        c = c
            .child(section("Corners"))
            .child(slider("radius", "Radius", st.radius, 0.0, 100.0 * unit, px(st.radius), accent, |st, v| st.radius = v));
    }
    if boxy {
        c = c.child(section("Hatching")).child(hatch_controls(st, custom, unit, accent));
    }
    c
}

fn text_tab(s: &Editor, a: &Annotation, unit: f32, accent: Color) -> El {
    let t = theme();
    let st = &a.style;
    let label = match a.kind {
        Kind::Text => "Text (Shift+Enter: new line)",
        Kind::Legend => "Title",
        Kind::Badge => "Number",
        _ => "Label",
    };
    let font_button = row()
        .key("font-button")
        .role("button")
        .label("Font")
        .w_full()
        .h(30.0)
        .px(10.0)
        .items_center()
        .radius(6.0)
        .fill(t.control_track)
        .hover(t.hover)
        .cursor(Cursor::Pointer)
        .child(text(font_name(st)).font(font::text_style(st, 13.0)).color(t.text).truncate().grow(1.0))
        .child(text(if s.font_menu { "▴" } else { "▾" }).font(type_::LABEL).color(t.text_secondary))
        .on_press(|s: &mut Editor| s.font_menu = !s.font_menu);
    let mut c = col().w_full().gap(10.0);
    if a.kind != Kind::Measure {
        c = c.child(section(label)).child(
            el().w_full().padding(8.0).radius(6.0).fill(t.bg).border(1.0, t.card_border).child(
                text_input(&s.caption, type_::BODY)
                    .key("caption")
                    .label("Annotation text")
                    .w_full()
                    .color(t.text)
                    .placeholder("Type some text…", t.text_secondary)
                    .on_edit(|s: &mut Editor| &mut s.caption),
            ),
        );
    }
    if a.kind == Kind::Legend {
        c = c.child(legend_rows(s, a, accent));
    }
    c = c.child(section("Font")).child(font_button);
    if s.font_menu {
        c = c.child(font_menu(s, st));
    }
    c = c
        .child(checkbox("bold", "Bold", st.bold, |st| st.bold = !st.bold))
        .child(slider("font-size", "Size", st.font_size, 4.0, 200.0 * unit, px(st.font_size), accent, |st, v| st.font_size = v))
        .child(section("Colour"))
        .child(swatches("text-color", st.text_color, false, &s.palette, Target::Text));
    if matches!(a.kind, Kind::Text | Kind::Measure | Kind::Legend) || st.show_size {
        c = c.child(slider("padding", "Padding", st.padding, 0.0, 60.0 * unit, px(st.padding), accent, |st, v| st.padding = v));
    }
    c
}

/// A legend's rows: a chip of each look and its description, editable in place.
fn legend_rows(s: &Editor, a: &Annotation, accent: Color) -> El {
    let t = theme();
    let mut c = col().w_full().gap(6.0).child(section("Entries"));
    for (i, (item, edit)) in a.legend.iter().zip(&s.legend_edits).enumerate() {
        let chip =
            if matches!(item.kind, Kind::Badge | Kind::Dot) || item.style.fill.0[3] > 120 { item.style.fill } else { item.style.stroke };
        c = c.child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(el().size(14.0, 14.0).radius(3.0).fill(chip.with_alpha(255).to_rune()).border(1.0, t.card_border))
                .child(
                    el().grow(1.0).min_w(0.0).padding(6.0).radius(6.0).fill(t.bg).border(1.0, t.card_border).child(
                        text_input(edit, type_::BODY)
                            .key(format!("legend-{i}"))
                            .label("Legend entry")
                            .w_full()
                            .color(t.text)
                            .placeholder("What this look means", t.text_secondary)
                            .on_edit(move |s: &mut Editor| &mut s.legend_edits[i]),
                    ),
                )
                .child(
                    el().key(format!("legend-remove-{i}"))
                        .role("button")
                        .label("Remove entry")
                        .size(24.0, 24.0)
                        .radius(5.0)
                        .center()
                        .hover(t.hover)
                        .cursor(Cursor::Pointer)
                        .child(text("×").font(type_::TITLE).color(t.text_secondary))
                        .on_press(move |s: &mut Editor| s.remove_legend_row(i)),
                ),
        );
    }
    c.child(button("legend-update", "Update from annotations", false, true, accent).on_press(|s: &mut Editor| s.update_legend()))
}

fn arrange_tab(accent: Color) -> El {
    col()
        .w_full()
        .gap(8.0)
        .child(section("Stacking order"))
        .child(pair(
            button("front", "Bring to front", false, true, accent).on_press(|s: &mut Editor| s.restack(true, true)),
            button("back", "Send to back", false, true, accent).on_press(|s: &mut Editor| s.restack(false, true)),
        ))
        .child(pair(
            button("forward", "Forward", false, true, accent).on_press(|s: &mut Editor| s.restack(true, false)),
            button("backward", "Backward", false, true, accent).on_press(|s: &mut Editor| s.restack(false, false)),
        ))
        .child(section("Annotation"))
        .child(pair(
            button("duplicate", "Duplicate", false, true, accent).on_press(|s: &mut Editor| s.duplicate_selected()),
            button("delete", "Delete", false, true, accent).on_press(|s: &mut Editor| s.delete_selected()),
        ))
}

/// Calibration: the selected measure's real length sets the image's scale for every measure.
fn scale_section(s: &Editor, a: &Annotation, accent: Color) -> El {
    let t = theme();
    let px = match a.shape {
        Geo::Arrow { from, to } => (to[0] - from[0]).hypot(to[1] - from[1]),
        _ => 0.0,
    };
    let status = match s.scale.mm_per_px {
        Some(_) => format!("This line: {}  ·  {} px", s.scale.format(px), px.round()),
        None => format!("Not calibrated: this line is {} px.", px.round()),
    };
    let units: Vec<_> = Unit::ALL.iter().map(|u| (*u, u.symbol().to_string())).collect();
    col()
        .w_full()
        .gap(8.0)
        .child(section("Scale"))
        .child(text(status).font(type_::LABEL).color(t.text_secondary).wrap())
        .child(segmented("unit", &units, s.calib_unit, accent, |s, u| s.set_unit(u)))
        .child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(
                    el().grow(1.0).padding(6.0).radius(6.0).fill(t.bg).border(1.0, t.card_border).child(
                        text_input(&s.calib_field, type_::BODY)
                            .key("calib-field")
                            .label("Real length")
                            .w_full()
                            .color(t.text)
                            .placeholder("Real length, such as 4.5", t.text_secondary)
                            .on_edit(|s: &mut Editor| &mut s.calib_field)
                            .on_submit(|s: &mut Editor, _| s.calibrate()),
                    ),
                )
                .child(button("calibrate", "Set scale", true, true, accent).on_press(|s: &mut Editor| s.calibrate())),
        )
        .children(
            s.scale
                .mm_per_px
                .is_some()
                .then(|| button("clear-scale", "Clear scale", false, true, accent).on_press(|s: &mut Editor| s.clear_scale())),
        )
}

/// A row of choices, one of them current.
fn segmented<T: Copy + PartialEq + 'static>(
    key: &'static str,
    options: &[(T, String)],
    current: T,
    accent: Color,
    pick: fn(&mut Editor, T),
) -> El {
    let t = theme();
    let mut r = row().w_full().gap(4.0);
    for (i, (v, name)) in options.iter().enumerate() {
        let (v, on) = (*v, *v == current);
        r = r.child(
            el().key(format!("{key}-{i}"))
                .role("radio")
                .label(name.clone())
                .checked(on)
                .grow(1.0)
                .h(28.0)
                .radius(6.0)
                .center()
                .fill(if on { accent } else { t.control_track })
                .hover(t.hover)
                .cursor(Cursor::Pointer)
                .child(text(name.clone()).font(type_::LABEL).color(if on { Color::WHITE } else { t.text }))
                .on_press(move |s: &mut Editor| pick(s, v)),
        );
    }
    r
}

const EXPORT_W: f32 = 300.0;

fn export_quality(o: &ExportOptions, accent: Color) -> El {
    let t = theme();
    let frac = (o.quality as f32 - 1.0) / 99.0;
    col()
        .w_full()
        .gap(2.0)
        .child(
            row()
                .w_full()
                .child(text(if o.format == Format::Pdf { "Photo quality" } else { "Quality" }).font(type_::BODY).color(t.text))
                .child(spacer())
                .child(text(o.quality.to_string()).font(type_::LABEL).color(t.text_secondary)),
        )
        .child(
            el().key("export-quality")
                .role("slider")
                .label("Quality")
                .w(EXPORT_W)
                .h(18.0)
                .cursor(Cursor::Pointer)
                .child(el().absolute(0.0, 7.0).size(EXPORT_W, 4.0).radius(2.0).fill(t.control_track).inert())
                .child(el().absolute(0.0, 7.0).size(frac * EXPORT_W, 4.0).radius(2.0).fill(accent).inert())
                .child(
                    el().absolute(frac * (EXPORT_W - 14.0), 2.0)
                        .size(14.0, 14.0)
                        .radius(7.0)
                        .fill(Color::WHITE)
                        .border(1.0, Color::BLACK.alpha(0.35))
                        .inert(),
                )
                .on_drag(|s: &mut Editor, d: &mut Drag, _| {
                    let v = ((d.x - 7.0) / (d.w - 14.0).max(1.0)).clamp(0.0, 1.0);
                    s.export.quality = (1.0 + v * 99.0).round() as u8;
                }),
        )
}

fn export_view(s: &Editor, accent: Color) -> El {
    let t = theme();
    let o = &s.export;
    let (iw, ih) = s.picture.as_ref().map_or((1, 1), |p| p.rgba.dimensions());
    let (w, h) = o.size((iw, ih));
    let formats: Vec<_> = Format::ALL.iter().map(|f| (*f, f.name().to_string())).collect();
    let scales: Vec<_> = [0.25f32, 0.5, 1.0, 2.0].iter().map(|k| (*k, format!("{} %", (k * 100.0).round()))).collect();
    let size_note = if o.format == Format::Pdf {
        format!("{} × {} pt page, photo at full resolution", (w as f32 * 0.75).round(), (h as f32 * 0.75).round())
    } else {
        format!("{w} × {h} px")
    };
    let note = match o.format {
        Format::Png => "Lossless, with transparency.",
        Format::Jpeg => "Smallest for photos. No transparency: the photo is always included.",
        Format::Webp => "Lossless and usually smaller than PNG, with transparency.",
        Format::Pdf => "Annotations stay vectors, and their text can be selected, searched and copied.",
    };
    let mut card = col()
        .key("export-dialog")
        .w(EXPORT_W + 32.0)
        .padding(16.0)
        .gap(12.0)
        .radius(12.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .shadow(12.0, 40.0, Color::BLACK.alpha(0.5))
        .child(text("Export").font(TextStyle::new(15.0, 600.0)).color(t.text))
        .child(section("Format"))
        .child(segmented("export-format", &formats, o.format, accent, |s, f| s.export.format = f))
        .child(text(note).font(type_::LABEL).color(t.text_secondary).wrap())
        .child(section("Size"))
        .child(segmented("export-scale", &scales, o.scale, accent, |s, k| s.export.scale = k))
        .child(text(size_note).font(type_::LABEL).color(t.text_secondary));
    if o.format == Format::Jpeg || (o.format == Format::Pdf && o.photo) {
        card = card.child(export_quality(o, accent));
    }
    if o.format.can_drop_photo() {
        card = card.child(check("export-photo", "Include the photo", o.photo).on_press(|s: &mut Editor| s.export.photo = !s.export.photo));
    }
    if o.format == Format::Png {
        card = card.child(
            check("export-small", "Smaller file (slower)", o.small_png).on_press(|s: &mut Editor| s.export.small_png = !s.export.small_png),
        );
    }
    card = card.child(
        row()
            .w_full()
            .gap(8.0)
            .child(spacer())
            .child(button("export-cancel", "Cancel", false, true, accent).on_press(|s: &mut Editor| s.export_open = false))
            .child(button("export-go", "Export…", true, true, accent).on_press_tx(export_dialog)),
    );
    el().key("export-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(Color::BLACK.alpha(0.45))
        .center()
        .child(card.modal(true, |s: &mut Editor| s.export_open = false))
}

fn stitch_view(s: &Editor, accent: Color) -> El {
    let t = theme();
    let st = &s.stitcher;
    let mut card = col()
        .key("stitch-dialog")
        .w(STITCH_W + 32.0)
        .padding(16.0)
        .gap(12.0)
        .radius(12.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .shadow(12.0, 40.0, Color::BLACK.alpha(0.5))
        .child(
            text(if st.mode == Mode::Stitch && st.stage != Stage::Setup { "Stitch images" } else { "Open" })
                .font(TextStyle::new(15.0, 600.0))
                .color(t.text),
        );
    match &st.stage {
        Stage::Setup => {
            let modes = [(Mode::Single, "One image".to_string()), (Mode::Stitch, "Stitch several".to_string())];
            card = card.child(segmented("open-mode", &modes, st.mode, accent, |s, m| s.stitcher.mode = m));
            if st.mode == Mode::Single {
                card = card
                    .child(text("Open one image to annotate. You can also drop it here.").font(type_::LABEL).color(t.text_secondary).wrap())
                    .child(
                        row()
                            .w_full()
                            .gap(8.0)
                            .child(spacer())
                            .child(button("stitch-cancel", "Cancel", false, true, accent).on_press(|s: &mut Editor| s.stitcher.close()))
                            .child(button("open-choose", "Choose an image…", true, true, accent).on_press_tx(choose_image)),
                    );
            } else {
                let shown = 8;
                let mut list = col().w_full().gap(4.0);
                for (i, p) in st.paths.iter().take(shown).enumerate() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    list = list.child(
                        row()
                            .w_full()
                            .gap(8.0)
                            .items_center()
                            .child(el().grow(1.0).min_w(0.0).child(text(name).font(type_::LABEL).color(t.text).truncate()))
                            .child(
                                button(format!("stitch-rm-{i}"), "×", false, true, accent)
                                    .on_press(move |s: &mut Editor| s.stitcher.remove(i)),
                            ),
                    );
                }
                let hidden = st.paths.len().saturating_sub(shown);
                if hidden > 0 {
                    list = list.child(text(format!("and {hidden} more")).font(type_::LABEL).color(t.text_secondary));
                }
                let summary = match st.paths.len() {
                    0 => "No images yet. Add two or more, or drop them here.".to_string(),
                    1 => "1 image. Add at least one more.".to_string(),
                    n => format!("{n} images"),
                };
                let note = match st.output {
                    Output::Editor => "Opens in the editor, up to 8192 px a side, ready to annotate.".to_string(),
                    Output::File => "As large as the photos allow (up to 200 megapixels), saved as PNG or WebP.".to_string(),
                };
                let outputs = [(Output::Editor, "Open in Layr".to_string()), (Output::File, "Save to a file".to_string())];
                card = card
                    .child(text("Photos are joined by what they have in common, in any order, turned or taken at an angle. Each should share about a third of its view with another.").font(type_::LABEL).color(t.text_secondary).wrap())
                    .child(section("Images"))
                    .child(text(summary).font(type_::LABEL).color(t.text_secondary))
                    .child(list)
                    .child(
                        row()
                            .w_full()
                            .gap(8.0)
                            .child(button("stitch-add", "Add images…", false, !st.picking(), accent).on_press(|s: &mut Editor| s.stitcher.pick_images()))
                            .child(button("stitch-folder", "Add a folder…", false, true, accent).on_press_tx(add_folder))
                            .child(spacer())
                            .child(button("stitch-clear", "Clear", false, !st.paths.is_empty(), accent).on_press(|s: &mut Editor| s.stitcher.paths.clear())),
                    )
                    .child(section("Result"))
                    .child(segmented("stitch-output", &outputs, st.output, accent, |s, o| s.stitcher.output = o))
                    .child(text(note).font(type_::LABEL).color(t.text_secondary).wrap())
                    .child(check("stitch-crop", "Crop to a rectangle", st.crop).on_press(|s: &mut Editor| s.stitcher.crop = !s.stitcher.crop))
                    .child(
                        text(if st.crop {
                            "Keeps the largest rectangle the photos fill, without the ragged edges of the assembly."
                        } else {
                            "Keeps everything; the area no photo covers stays transparent."
                        })
                        .font(type_::LABEL)
                        .color(t.text_secondary)
                        .wrap(),
                    )
                    .child(
                        row()
                            .w_full()
                            .gap(8.0)
                            .child(spacer())
                            .child(button("stitch-cancel", "Cancel", false, true, accent).on_press(|s: &mut Editor| s.stitcher.close()))
                            .child(button("stitch-go", "Stitch", true, st.can_start(), accent).on_press(|s: &mut Editor| s.stitcher.start())),
                    );
            }
        }
        Stage::Running => {
            let (stage, frac) = st.status().unwrap_or_default();
            card = card
                .child(text(if stage.is_empty() { "Starting…".to_string() } else { format!("{stage}…") }).font(type_::LABEL).color(t.text))
                .child(
                    el().key("stitch-bar")
                        .w(STITCH_W)
                        .h(6.0)
                        .child(el().absolute(0.0, 0.0).size(STITCH_W, 6.0).radius(3.0).fill(t.control_track).inert())
                        .child(el().absolute(0.0, 0.0).size((frac * STITCH_W).max(6.0), 6.0).radius(3.0).fill(accent).inert()),
                )
                .child(
                    row()
                        .w_full()
                        .child(spacer())
                        .child(button("stitch-stop", "Cancel", false, true, accent).on_press(|s: &mut Editor| s.stitcher.close())),
                );
        }
        Stage::Failed(e) => {
            card = card.child(text(e.clone()).font(type_::LABEL).color(t.text).wrap()).child(
                row()
                    .w_full()
                    .child(spacer())
                    .child(button("stitch-close", "Close", true, true, accent).on_press(|s: &mut Editor| s.stitcher.close())),
            );
        }
        Stage::Idle => {}
    }
    el().key("stitch-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(Color::BLACK.alpha(0.45))
        .center()
        // While a modal is open only it takes drops, so the menu itself does.
        .child(
            card.on_drop_files(|s: &mut Editor, files, _| drop_files(s, files))
                .accepts_extensions(&IMAGE_EXTENSIONS)
                .modal(true, |s: &mut Editor| s.stitcher.close()),
        )
}

fn properties(s: &Editor, accent: Color) -> El {
    let t = theme();
    let Some(a) = s.selection() else {
        return col().w_full().gap(8.0).child(text("Nothing selected").font(TextStyle::new(15.0, 600.0)).color(t.text_secondary));
    };
    let unit = s.picture.as_ref().map_or(1.0, |p| p.unit());
    let has_text = matches!(a.kind, Kind::Rect | Kind::Hatch | Kind::Badge | Kind::Text | Kind::Measure | Kind::Legend);
    let tab = if s.panel_tab == PanelTab::Text && !has_text { PanelTab::Style } else { s.panel_tab };
    let body = match tab {
        PanelTab::Style => style_tab(s, a, unit, accent),
        PanelTab::Text => text_tab(s, a, unit, accent),
        PanelTab::Arrange => arrange_tab(accent),
    };
    col()
        .w_full()
        .gap(12.0)
        .child(text(a.kind.name()).font(TextStyle::new(15.0, 600.0)).color(t.text))
        .child(tabs(tab, has_text, accent))
        .child(body)
}

// ---- Colour editor ----------------------------------------------------------------------------

/// A draggable bar picking one of the editor's channels from 0 to 1.
fn channel(key: &'static str, label: &str, value: f32, stops: Vec<(f32, Color)>, set: fn(&mut ColorTool, f32)) -> El {
    let t = theme();
    let knob_x = value.clamp(0.0, 1.0) * PICKER_W - 7.0;
    col().w_full().gap(4.0).child(text(label).font(type_::LABEL).color(t.text_secondary)).child(
        el().key(key)
            .w(PICKER_W)
            .h(18.0)
            .radius(9.0)
            .linear_gradient(FRAC_PI_2, stops)
            .border(1.0, t.card_border)
            .cursor(Cursor::Pointer)
            .child(el().absolute(knob_x, 2.0).size(14.0, 14.0).radius(7.0).fill(Color::WHITE).border(2.0, Color::BLACK.alpha(0.6)).inert())
            .on_drag(move |s: &mut Editor, d: &mut Drag, _| {
                if let Some(tool) = &mut s.color_tool {
                    set(tool, (d.x / d.w.max(1.0)).clamp(0.0, 1.0));
                    tool.write_field();
                }
            }),
    )
}

fn color_editor(s: &Editor, tool: &ColorTool, accent: Color) -> El {
    let t = theme();
    let c = tool.color();
    let rgb = |h: f32, s: f32, l: f32| {
        let [r, g, b] = color::hsl_to_rgb(h, s, l);
        Rgba([r, g, b, 255]).to_rune()
    };
    let hue: Vec<(f32, Color)> = (0..7).map(|i| (i as f32 / 6.0, rgb(i as f32 * 60.0, 1.0, 0.5))).collect();
    let sat = vec![(0.0, rgb(tool.h, 0.0, tool.l)), (1.0, rgb(tool.h, 1.0, tool.l))];
    let light = vec![(0.0, Color::BLACK), (0.5, rgb(tool.h, tool.s, 0.5)), (1.0, Color::WHITE)];
    let alpha = vec![(0.0, c.with_alpha(0).to_rune()), (1.0, c.with_alpha(255).to_rune())];

    let mut picks = Vec::new();
    for (i, sw) in PALETTE.iter().chain(&s.palette).copied().enumerate() {
        picks.push(swatch(format!("pick-{i}"), sw, c.same_hue(sw)).on_press(move |s: &mut Editor| {
            if let Some(tool) = &mut s.color_tool {
                let a = tool.color().0[3];
                tool.set_color(sw.with_alpha(a));
            }
        }));
    }

    let card = col()
        .key("color-editor")
        .w(PICKER_W + 32.0)
        .padding(16.0)
        .gap(12.0)
        .radius(12.0)
        .fill(t.card)
        .border(1.0, t.card_border)
        .shadow(12.0, 40.0, Color::BLACK.alpha(0.5))
        .child(text(tool.target.name()).font(TextStyle::new(15.0, 600.0)).color(t.text))
        .child(
            row()
                .w_full()
                .gap(10.0)
                .items_center()
                .child(el().size(48.0, 48.0).radius(8.0).fill(c.to_rune()).border(1.0, t.card_border))
                .child(
                    col()
                        .grow(1.0)
                        .gap(4.0)
                        .child(
                            el().w_full()
                                .padding(8.0)
                                .radius(6.0)
                                .fill(t.bg)
                                .border(1.0, if tool.invalid { rune::color::hex("#E5484D").unwrap_or(Color::WHITE) } else { t.card_border })
                                .child(
                                    text_input(&tool.field, type_::BODY)
                                        .key("color-field")
                                        .label("Colour")
                                        .w_full()
                                        .color(t.text)
                                        .placeholder("#RRGGBB, rgb(), hsl() or a name", t.text_secondary)
                                        .on_edit(|s: &mut Editor| &mut s.color_tool.as_mut().expect("colour editor is open").field),
                                ),
                        )
                        .children(tool.invalid.then(|| {
                            text("Not a colour: try #3FA7D6, rgb(63 167 214), hsl(200 65% 54%) or tomato.")
                                .font(type_::LABEL)
                                .color(t.text_secondary)
                                .wrap()
                        })),
                ),
        )
        .child(channel("hue", "Hue", tool.h / 360.0, hue, |t, v| t.h = v * 360.0))
        .child(channel("saturation", "Saturation", tool.s, sat, |t, v| t.s = v))
        .child(channel("lightness", "Lightness", tool.l, light, |t, v| t.l = v))
        .child(channel("alpha", "Opacity", tool.a, alpha, |t, v| t.a = v))
        .child(section("Palette"))
        .child(grid(picks, 11))
        .child(pair(
            button("random-flat", "Random flat colour", false, true, accent).on_press(|s: &mut Editor| {
                if let Some(tool) = &mut s.color_tool {
                    let a = tool.color().0[3];
                    if let Some(c) = color::random_flat(1).first() {
                        tool.set_color(c.with_alpha(a));
                    }
                }
            }),
            button("random-palette", "Add 8 to palette", false, true, accent).on_press(|s: &mut Editor| s.add_random_colors(8)),
        ))
        .child(pair(
            button("save-color", "Save to palette", false, true, accent).on_press(|s: &mut Editor| {
                if let Some(c) = s.color_tool.as_ref().map(ColorTool::color) {
                    s.add_to_palette(c);
                }
            }),
            button("clear-palette", "Clear my colours", false, !s.palette.is_empty(), accent).on_press(|s: &mut Editor| s.clear_palette()),
        ))
        .child(
            row()
                .w_full()
                .gap(8.0)
                .child(spacer())
                .child(button("color-cancel", "Cancel", false, true, accent).on_press(|s: &mut Editor| s.color_tool = None))
                .child(button("color-apply", "Apply", true, true, accent).on_press(|s: &mut Editor| s.apply_color_tool())),
        );
    el().key("color-scrim")
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .fill(Color::BLACK.alpha(0.45))
        .center()
        .child(card.modal(true, |s: &mut Editor| s.color_tool = None))
}

// ---- Updates ----------------------------------------------------------------------------------

/// A bar above the status line when a newer release exists, with what can be done about it.
fn update_banner(s: &Editor, accent: Color) -> Option<El> {
    let t = theme();
    let open_page = |s: &mut Editor| {
        if let Some(url) = s.updater.page() {
            update::open_url(&url);
        }
    };
    let release = button("update-page", "Release page", false, true, accent).on_press(open_page);
    let close = button("update-close", "Dismiss", false, true, accent).on_press(|s: &mut Editor| s.updater.dismiss());
    let (message, buttons): (String, Vec<El>) = match &s.updater.state {
        State::Available(o) if o.can_install() => (
            format!("Layr {} is available (you have {}).", o.release.version, update::VERSION),
            vec![button("update-install", "Update now", true, true, accent).on_press(|s: &mut Editor| s.updater.install()), release, close],
        ),
        State::Available(o) => (
            format!("Layr {} is available (you have {}). Download it from its release page.", o.release.version, update::VERSION),
            vec![button("update-page", "Open release page", true, true, accent).on_press(open_page), close],
        ),
        State::Downloading { offer, done, total } => {
            let pct = if *total > 0 { format!(" {} %", (done * 100 / total).min(100)) } else { String::new() };
            (format!("Downloading Layr {}…{pct}", offer.release.version), Vec::new())
        }
        State::Installing(offer) => (format!("Installing Layr {}… the app restarts by itself.", offer.release.version), Vec::new()),
        State::Failed { offer, message } => (
            format!("Couldn't update to Layr {}: {message}", offer.release.version),
            vec![button("update-page", "Open release page", true, true, accent).on_press(open_page), close],
        ),
        State::Idle | State::Checking => return None,
    };
    Some(
        row()
            .key("update-banner")
            .w_full()
            .h(40.0)
            .px(12.0)
            .gap(8.0)
            .items_center()
            .fill(t.card)
            .border(1.0, t.card_border)
            .child(el().grow(1.0).min_w(0.0).child(text(message).font(type_::LABEL).color(t.text).truncate()))
            .children(buttons),
    )
}

// ---- Window -----------------------------------------------------------------------------------

/// Files dropped on the window: one image opens; several are photos to stitch, which join the
/// open menu's list when it is showing.
fn drop_files(s: &mut Editor, files: &[std::path::PathBuf]) {
    let images: Vec<_> = files.iter().filter(|f| stitching::is_image(f)).cloned().collect();
    let menu = s.stitcher.stage == Stage::Setup;
    if menu && (images.len() >= 2 || s.stitcher.mode == Mode::Stitch) {
        s.stitcher.mode = Mode::Stitch;
        s.stitcher.add(images);
    } else if images.len() >= 2 {
        s.stitcher.begin(images);
    } else if let Some(f) = files.first() {
        s.stitcher.close();
        s.load(f);
    }
}

pub fn view(s: &Editor, cx: &mut Cx<Editor>) -> El {
    // Panels, toggles and dialogs change in place, without springs or slides.
    cx.reduced_motion(true);
    let t = theme();
    let accent = cx.accent();
    let panel = col()
        .key("panel")
        .w(PANEL_W)
        .h_full()
        .min_h(0.0)
        .fill(t.card)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(col().w_full().padding(14.0).child(properties(s, accent)));
    let status = row()
        .key("status")
        .w_full()
        .h(26.0)
        .px(12.0)
        .items_center()
        .fill(t.card)
        .gap(8.0)
        .child(el().grow(1.0).min_w(0.0).child(text(&s.status).font(type_::LABEL).color(t.text_secondary).truncate()))
        .child(
            row()
                .key("version")
                .role("button")
                .label("Check for updates")
                .items_center()
                .cursor(Cursor::Pointer)
                .child(text(format!("Layr {}", update::VERSION)).font(type_::LABEL).color(t.text_secondary))
                .on_press(|s: &mut Editor| s.updater.check(true)),
        );
    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(t.bg)
        .instant_changes()
        .on_drop_files(|s: &mut Editor, files, _| drop_files(s, files))
        .accepts_extensions(&IMAGE_EXTENSIONS)
        .on_key_up(|s: &mut Editor, e: &KeyEvent, _| {
            let space = e.key == Key::Text(" ".into());
            if space {
                s.space = false;
            }
            space
        })
        .child(
            row()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(rail(s, accent))
                .child(el().w(1.0).h_full().fill(t.separator))
                .child(viewport(s, accent))
                .children(s.selection().is_some().then(|| el().w(1.0).h_full().fill(t.separator)))
                .children(s.selection().is_some().then_some(panel)),
        )
        .children(update_banner(s, accent))
        .child(status);
    if let Some(tool) = &s.color_tool {
        root = root.child(color_editor(s, tool, accent));
    }
    if s.export_open && s.picture.is_some() {
        root = root.child(export_view(s, accent));
    }
    if s.stitcher.is_open() {
        root = root.child(stitch_view(s, accent));
    }
    root
}

/// Whether a text field has the keyboard, so letters type rather than pick tools.
fn typing(tx: &Tx) -> bool {
    tx.focused().is_some_and(|k| {
        k.contains("caption")
            || k.contains("color-field")
            || k.contains("font-search")
            || k.contains("calib-field")
            || k.contains("legend-")
    })
}

pub fn on_key(s: &mut Editor, e: &KeyEvent, tx: &mut Tx) -> bool {
    let typing = typing(tx);
    let shift = e.mods.shift;
    match &e.key {
        Key::Escape if s.color_tool.is_some() => s.color_tool = None,
        Key::Enter if s.color_tool.is_some() => s.apply_color_tool(),
        Key::Escape if s.font_menu => s.font_menu = false,
        Key::Escape if s.stitcher.is_open() => s.stitcher.close(),
        Key::Enter if s.stitcher.can_start() => s.stitcher.start(),
        Key::Escape if s.export_open => s.export_open = false,
        Key::Enter if s.export_open => export_dialog(s, tx),
        Key::Command(c) => match c {
            'z' if shift => s.redo(),
            'z' => s.undo(),
            'y' => s.redo(),
            'o' => s.stitcher.open(),
            'e' | 's' if s.picture.is_some() => s.export_open = true,
            'c' => s.copy(),
            'v' => s.paste(None),
            'f' => s.fit(),
            'd' => s.duplicate_selected(),
            '0' => s.fit(),
            '=' | '+' => s.zoom_center(1.25),
            '-' => s.zoom_center(1.0 / 1.25),
            ']' => s.restack(true, true),
            '[' => s.restack(false, true),
            _ => return false,
        },
        Key::Escape if typing => tx.blur(),
        _ if typing || s.color_tool.is_some() || s.export_open || s.stitcher.is_open() => return false,
        Key::Delete | Key::Backspace => s.delete_selected(),
        Key::Escape => s.escape(),
        Key::Enter => s.finish_wire(),
        Key::Left => s.nudge(if shift { -10.0 } else { -1.0 }, 0.0),
        Key::Right => s.nudge(if shift { 10.0 } else { 1.0 }, 0.0),
        Key::Up => s.nudge(0.0, if shift { -10.0 } else { -1.0 }),
        Key::Down => s.nudge(0.0, if shift { 10.0 } else { 1.0 }),
        Key::Text(t) => match t.as_str() {
            " " => s.space = true,
            "+" | "=" => s.zoom_center(1.25),
            "-" => s.zoom_center(1.0 / 1.25),
            "]" => s.restack(true, false),
            "[" => s.restack(false, false),
            "g" | "G" if s.picture.is_some() => s.add_legend(),
            other => match Tool::from_shortcut(other) {
                Some(tool) if s.picture.is_some() => s.set_tool(tool),
                _ => return false,
            },
        },
        _ => return false,
    }
    true
}
