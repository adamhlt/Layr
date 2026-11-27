//! The window: tool rail, canvas, properties panel, colour editor and status bar, drawn with the
//! Basalt design system (`rune-basalt`): its tokens, type and components.

use std::f32::consts::{FRAC_PI_2, PI};

use rune::file_dialog::FileDialogOptions;
use rune::{
    Align, Color, Cursor, Cx, Drag, El, Icon, Key, KeyEvent, LineCap, MagnifyEvent, Phase, PointerButton, ScrollSpec, Shape, Side,
    TextEdit, TextStyle, Tx, WheelEvent, canvas, col, el, icon, row, spacer, text, text_input,
};
use rune_basalt::app::{self, CAPTION, Kind, Level, R, TINY, TITLE, UI, UI_STRONG, caption};
use rune_basalt::icons::oc;
use rune_basalt::{SANS, Theme, Tokens};

use crate::color::{self, ColorTool, Target};
use crate::editor::{CROP_BAR_H, CROP_BAR_TOP, Editor, Hover, PanelTab, Tool, crop_handles};
use crate::measure::Unit;
use crate::model::{Annotation, BadgeShape, Hatch, Kind as AnnKind, PALETTE, Rgba, Shape as Geo, Style};
use crate::render::{Area, ExportOptions, Format};
use crate::stitch::job::{self as stitching, Mode, Output, Stage};
use crate::update::{self, State};
use crate::{cursors, font, vector};

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];
const PANEL_W: f32 = 290.0;
/// The panel's side padding.
const PANEL_PAD: f32 = 14.0;
const RAIL_W: f32 = 208.0;
/// Rail rows: a little tighter than Basalt's 30-point controls, so every tool fits the
/// smallest window.
const RAIL_ROW: f32 = 26.0;
/// The colour editor's inner width.
const PICKER_W: f32 = 340.0;
/// The stitching dialog's inner width.
const STITCH_W: f32 = 360.0;
/// The export dialog's inner width.
const EXPORT_W: f32 = 320.0;
/// Dialogs' distance from the window's top.
const DIALOG_TOP: f32 = 56.0;
/// Dialogs' padding.
const DIALOG_PAD: f32 = 20.0;
/// The rail's group captions: IBM Plex Sans SemiBold 13, in sentence case.
const RAIL_CAPTION: TextStyle = TextStyle::new(13.0, 600.0).family(rune::Family::Named("IBM Plex Sans")).lh(17.0);
/// Dialog titles: grotesk 18, as Basalt's dialogs.
const DIALOG_TITLE: TextStyle = TextStyle::new(18.0, 500.0).family(SANS);

/// The theme's colours. Views set the theme at the top of [`view`].
fn tk() -> Tokens {
    rune_basalt::tokens()
}

// ---- Small controls ---------------------------------------------------------------------------

/// Why most actions are unavailable before an image is open.
const NO_IMAGE: &str = "Open an image first";

/// The look and press of a hand-built control that can or can't be used now.
fn usable(b: El, enabled: bool) -> El {
    // `inert` is what stops the press, which would otherwise set state that takes effect once
    // the action becomes available.
    if enabled {
        b.focusable(true).hover_fill(tk().tint).cursor(Cursor::Pointer)
    } else {
        b.dim(0.4).aria_disabled(true).focusable(false).inert()
    }
}

/// Explains an unavailable control: `b` passes the pointer through, so a wrapper around it shows
/// `why` on hover, with a "not allowed" cursor, and takes the press without acting. `full`
/// stretches the wrapper across its column.
fn unavailable(b: El, why: Option<&str>, full: bool) -> El {
    let Some(why) = why else { return b };
    let w = row().help(why).cursor(Cursor::NotAllowed).child(b.inert());
    if full { w.w_full() } else { w }
}

/// A Basalt push button, greyed out with the reason as its tooltip when `why` is set.
fn button(key: impl Into<String>, label: &str, kind: Kind, why: Option<&str>, on_press: impl Fn(&mut Editor) + 'static) -> El {
    let b = app::disabled(why.is_some(), || app::button(label, kind, None, None, on_press)).key(key);
    unavailable(b, why, false)
}

/// A Basalt push button whose press needs the transaction (file dialogs).
fn button_tx(key: impl Into<String>, label: &str, kind: Kind, on_press: fn(&mut Editor, &mut Tx)) -> El {
    app::button(label, kind, None, None, |_: &mut Editor| {}).key(key).on_press_tx(on_press)
}

/// A small square icon button (status bar, list rows).
fn mini_button(key: impl Into<String>, glyph: &'static Icon, label: &str, size: f32, enabled: bool) -> El {
    let s = tk();
    let b = row().key(key).role("button").label(label).size(size, size).center().radius(R - 3.0).child(icon(glyph, 13.0).color(s.muted));
    usable(b, enabled)
}

fn section(title: &str) -> El {
    caption(title)
}

/// A field's box in Basalt's look (well fill, line border, focus ring) around `input`.
fn field(input: El, invalid: bool) -> El {
    let s = tk();
    let border = if invalid { s.danger } else { s.line };
    row()
        .w_full()
        .min_w(0.0)
        .px(10.0)
        .py(6.0)
        .min_h(app::h())
        .items_center()
        .radius(R)
        .fill(s.bg)
        .border(1.0, border)
        .hover_border(if invalid { s.danger } else { s.hair })
        .focus_within_ring(1.5, 0.0, if invalid { s.danger } else { s.text.alpha(0.5) })
        .child(input)
}

/// A text input in Basalt's control type and colours.
fn input(edit: &TextEdit, key: impl Into<String>, label: &str, placeholder: &str) -> El {
    let s = tk();
    text_input(edit, UI).key(key).label(label).w_full().color(s.text).caret(s.text, s.text.alpha(0.2)).placeholder(placeholder, s.dim)
}

fn swatch(key: String, c: Rgba, on: bool) -> El {
    let s = tk();
    el().key(key)
        .size(22.0, 22.0)
        .radius(6.0)
        .fill(c.with_alpha(255).to_rune())
        .border(if on { 2.0 } else { 1.0 }, if on { s.text } else { s.hair })
        .cursor(Cursor::Pointer)
}

/// The built-in colours, the user's own, and a button opening the colour editor on `target`.
fn swatches(key: &'static str, current: Rgba, none: bool, custom: &[Rgba], target: Target) -> El {
    let s = tk();
    let mut items = Vec::new();
    if none {
        let on = current.0[3] == 0;
        items.push(
            el().key(format!("{key}-none"))
                .size(22.0, 22.0)
                .radius(6.0)
                .fill(s.bg)
                .border(if on { 2.0 } else { 1.0 }, if on { s.text } else { s.hair })
                .child(text("∅").font(UI).color(s.muted))
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
            .role("button")
            .label("More colours")
            .size(22.0, 22.0)
            .radius(6.0)
            .fill(s.tint)
            .hover_fill(s.text.alpha(0.14))
            .center()
            .cursor(Cursor::Pointer)
            .child(icon(&oc::PLUS_SMALL, 14.0).color(s.text))
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
const SLIDER_W: f32 = PANEL_W - 2.0 * PANEL_PAD;

/// A labelled slider in Basalt's look, `w` wide at `frac` (0–1). `on_drag` gets the new
/// fraction and whether the drag just started.
fn slider_track(
    key: &'static str,
    label: &str,
    frac: f32,
    shown: String,
    w: f32,
    on_drag: impl Fn(&mut Editor, f32, bool) + 'static,
) -> El {
    let s = tk();
    let frac = frac.clamp(0.0, 1.0);
    let knob = 14.0;
    col()
        .w_full()
        .gap(2.0)
        .child(
            row()
                .w_full()
                .items_center()
                .child(text(label).font(UI).color(s.muted))
                .child(spacer())
                .child(text(shown).key(format!("{key}-value")).font(UI).color(s.text)),
        )
        .child(
            el().key(key)
                .role("slider")
                .label(label)
                .w(w)
                .h(20.0)
                .cursor(Cursor::Pointer)
                .child(el().absolute(0.0, 8.0).size(w, 4.0).radius(2.0).fill(s.line).inert())
                .child(el().absolute(0.0, 8.0).size(frac * w, 4.0).radius(2.0).fill(s.text).inert())
                .child(el().absolute(frac * (w - knob), 3.0).size(knob, knob).radius(knob * 0.5).fill(s.solid).border(3.0, s.panel).inert())
                .on_drag(move |s: &mut Editor, d: &mut Drag, _| {
                    let v = ((d.x - knob * 0.5) / (d.w - knob).max(1.0)).clamp(0.0, 1.0);
                    on_drag(s, v, d.phase == Phase::Down);
                }),
        )
}

/// A slider over `min..=max` for one style value. A drag is one undo step.
fn slider(key: &'static str, label: &str, value: f32, min: f32, max: f32, shown: String, set: fn(&mut Style, f32)) -> El {
    let frac = (value - min) / (max - min).max(f32::EPSILON);
    slider_track(key, label, frac, shown, SLIDER_W, move |s, v, start| s.slide_style(start, |st| set(st, min + (max - min) * v)))
}

fn checkbox(key: &'static str, label: &str, on: bool, toggle: fn(&mut Style)) -> El {
    app::checkbox(label, on, move |s: &mut Editor| s.edit_style(toggle)).key(key)
}

/// Two equal buttons a row: wrapping rows overflow a scroll view's width.
fn pair(a: El, b: El) -> El {
    row().w_full().gap(6.0).child(a.grow(1.0).center()).child(b.grow(1.0).center())
}

/// A row of choices, one of them current, in Basalt's segmented look, stretched across its
/// column.
fn segmented<T: Copy + PartialEq + 'static>(key: &'static str, options: &[(T, String)], current: T, pick: fn(&mut Editor, T)) -> El {
    let items = options.iter().enumerate().map(|(i, (v, name))| {
        let v = *v;
        (format!("{key}-{i}"), name.clone(), None, v == current, move |s: &mut Editor| pick(s, v))
    });
    segments(items, app::h())
}

/// Segments of a segmented control: key, label, optional drawing in place of the label,
/// whether it is current, and what pressing it does.
fn segments<F: Fn(&mut Editor) + 'static>(items: impl Iterator<Item = (String, String, Option<El>, bool, F)>, height: f32) -> El {
    let s = tk();
    let mut r = row().w_full().h(height).padding(3.0).gap(2.0).radius(R).fill(s.bg).border(1.0, s.line).role("radiogroup").focus_group();
    for (key, name, drawing, on, press) in items {
        let mut b = row()
            .key(key)
            .role("radio")
            .label(name.clone())
            .checked(on)
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .px(6.0)
            .center()
            .radius(R - 3.0)
            .focusable(true)
            .cursor(Cursor::Pointer);
        // Hovered, an unchosen segment only lights its label up: no veil.
        b = if on { b.fill(s.tint).border(1.0, s.hair) } else { b.hover_group() };
        b = match drawing {
            // A drawing can't change colour on hover: it is drawn in the text colour, dimmed to
            // about the muted grey until hovered.
            Some(d) => b.child(if on { d } else { d.group_opacity(0.55, 1.0) }),
            None => {
                let label = text(name).font(UI).color(if on { s.text } else { s.muted }).truncate();
                b.child(if on { label } else { label.group_color(s.text) })
            }
        };
        r = r.child(b.on_press(press));
    }
    r
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

// ---- Dialogs ----------------------------------------------------------------------------------

/// A dialog's floating card, `w` wide inside its padding, with its title.
fn dialog_card(key: &'static str, w: f32, title: &str) -> El {
    app::floating(col().key(key).w(w + 2.0 * DIALOG_PAD).padding(DIALOG_PAD).gap(12.0))
        .role("dialog")
        .label(title)
        .child(text(title).font(DIALOG_TITLE).color(tk().text))
}

/// `card` over the dimmed window, modal: Escape or a press outside closes it.
fn dialog_scrim(key: &'static str, card: El, close: fn(&mut Editor)) -> El {
    // Centred across, at a fixed distance from the top: a dialog whose content changes (a tab,
    // an option) grows downwards instead of moving its top.
    col().key(key).absolute(0.0, 0.0).w_full().h_full().fill(tk().scrim).items_center().pt(DIALOG_TOP).child(card.modal(true, close))
}

/// A dialog's closing row: something on the left, the buttons on the right.
fn dialog_actions(left: Option<El>, buttons: Vec<El>) -> El {
    row().w_full().pt(6.0).gap(8.0).items_center().children(left).child(spacer()).children(buttons)
}

/// Explanatory copy under a control.
fn note(t: impl Into<String>) -> El {
    text(t.into()).font(TINY).color(tk().muted).wrap()
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
        // A cross with an arrowhead on each end.
        Tool::Move => Shape::new()
            .line(k(9.0, 2.0), k(9.0, 16.0), 1.6, LineCap::Round, c)
            .line(k(2.0, 9.0), k(16.0, 9.0), 1.6, LineCap::Round, c)
            .fill(vec![k(9.0, 0.5), k(11.8, 3.8), k(6.2, 3.8)], c)
            .fill(vec![k(9.0, 17.5), k(6.2, 14.2), k(11.8, 14.2)], c)
            .fill(vec![k(0.5, 9.0), k(3.8, 6.2), k(3.8, 11.8)], c)
            .fill(vec![k(17.5, 9.0), k(14.2, 11.8), k(14.2, 6.2)], c),
        Tool::Draw(AnnKind::Rect) => {
            Shape::new().stroke_with(vec![k(2.0, 4.0), k(16.0, 4.0), k(16.0, 14.0), k(2.0, 14.0)], true, 1.6, LineCap::Round, c)
        }
        Tool::Draw(AnnKind::Hatch) => {
            let mut sh =
                Shape::new().stroke_with(vec![k(2.0, 3.0), k(16.0, 3.0), k(16.0, 15.0), k(2.0, 15.0)], true, 1.4, LineCap::Round, c);
            for (a, b) in
                [(k(2.0, 9.0), k(8.0, 3.0)), (k(2.0, 15.0), k(14.0, 3.0)), (k(8.0, 15.0), k(16.0, 7.0)), (k(14.0, 15.0), k(16.0, 13.0))]
            {
                sh = sh.line(a, b, 1.0, LineCap::Butt, c);
            }
            sh
        }
        Tool::Draw(AnnKind::Badge) => {
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
        Tool::Draw(AnnKind::Wire) => {
            Shape::new().stroke(vec![k(2.0, 3.0), k(2.0, 9.0), k(9.0, 13.0), k(16.0, 13.0), k(16.0, 16.0)], 1.8, c)
        }
        Tool::Draw(AnnKind::Arrow) => {
            Shape::new().line(k(3.0, 15.0), k(13.0, 5.0), 1.8, LineCap::Round, c).fill(vec![k(16.0, 2.0), k(14.5, 9.0), k(9.0, 3.5)], c)
        }
        Tool::Draw(AnnKind::Dot) => Shape::new().fill(
            (0..16)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 16.0;
                    k(9.0 + 5.0 * a.cos(), 9.0 + 5.0 * a.sin())
                })
                .collect::<Vec<_>>(),
            c,
        ),
        Tool::Draw(AnnKind::Text) => {
            Shape::new().line(k(3.0, 3.0), k(15.0, 3.0), 2.0, LineCap::Round, c).line(k(9.0, 3.0), k(9.0, 16.0), 2.0, LineCap::Round, c)
        }
        // Legends come from the rail's Legend button, not a drawing tool.
        Tool::Draw(AnnKind::Legend) => Shape::new(),
        Tool::Draw(AnnKind::Measure) => Shape::new()
            .line(k(2.0, 12.0), k(16.0, 12.0), 1.6, LineCap::Butt, c)
            .line(k(2.0, 8.0), k(2.0, 16.0), 1.6, LineCap::Butt, c)
            .line(k(16.0, 8.0), k(16.0, 16.0), 1.6, LineCap::Butt, c)
            .line(k(6.5, 10.0), k(6.5, 12.0), 1.2, LineCap::Butt, c)
            .line(k(11.5, 10.0), k(11.5, 12.0), 1.2, LineCap::Butt, c)
            .line(k(9.0, 9.0), k(9.0, 12.0), 1.2, LineCap::Butt, c),
    };
    canvas(shape).size(18.0, 18.0)
}

/// Two overlapping right angles: the crop marks.
fn crop_icon(c: Color) -> El {
    let k = |x: f32, y: f32| [x, y];
    canvas(Shape::new().stroke_with(vec![k(5.0, 1.5), k(5.0, 13.0), k(16.5, 13.0)], false, 1.6, LineCap::Round, c).stroke_with(
        vec![k(1.5, 5.0), k(13.0, 5.0), k(13.0, 16.5)],
        false,
        1.6,
        LineCap::Round,
        c,
    ))
    .size(18.0, 18.0)
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

/// An OpenCode icon in the rail's 18-point slot.
fn glyph(g: &'static Icon, c: Color) -> El {
    row().size(18.0, 18.0).center().child(icon(g, 15.0).color(c))
}

/// Rail ink: lit for the current tool, muted otherwise (Basalt's sidebar items).
fn ink(on: bool) -> Color {
    let s = tk();
    if on { s.text } else { s.muted }
}

/// A rail item, as Basalt's sidebar items: icon, label, shortcut, lit when `active`.
fn rail_button(key: impl Into<String>, icon: El, label: &str, hint: &str, active: bool, enabled: bool) -> El {
    let s = tk();
    let b = row()
        .key(key)
        .role("button")
        .label(label)
        .checked(active)
        .w_full()
        .h(RAIL_ROW)
        .px(8.0)
        .gap(10.0)
        .items_center()
        .radius(R)
        .fill(if active { s.tint } else { Color::TRANSPARENT })
        .child(icon)
        .child(text(label).font(UI).color(ink(active)))
        .child(spacer())
        .child(keycaps(hint));
    usable(b, enabled)
}

/// A shortcut as shown: capitals with ` + ` between the keys, `Ctrl+O` as `CTRL + O`. On macOS
/// the command key stands for Ctrl (`⌘ + O`) and Delete is `⌫`.
fn shortcut_text(shortcut: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let keys: Vec<String> = shortcut
        .split('+')
        .map(|k| match k {
            "Ctrl" if mac => "⌘".to_string(),
            "Del" if mac => "⌫".to_string(),
            k => k.to_uppercase(),
        })
        .collect();
    keys.join(" + ")
}

/// A shortcut in one Basalt keycap.
fn keycaps(shortcut: &str) -> El {
    app::kbd(&shortcut_text(shortcut))
}

/// A caption over a group of rail items.
fn rail_caption(title: &str, first: bool) -> El {
    row().w_full().px(8.0).pt(if first { 2.0 } else { 10.0 }).pb(4.0).child(text(title).font(RAIL_CAPTION).color(tk().muted))
}

fn rail(s: &Editor) -> El {
    let t = tk();
    let has = s.picture.is_some();
    let no_image = (!has).then_some(NO_IMAGE);
    let mut tools = col().w_full().gap(2.0);
    for (tool, name, key) in Tool::ALL {
        let on = s.tool == tool && !s.crop_image;
        tools = tools.child(unavailable(
            rail_button(format!("tool-{key}"), tool_icon(tool, ink(on)), name, key, on, has)
                .on_press(move |s: &mut Editor| s.set_tool(tool)),
            no_image,
            true,
        ));
    }
    let crop_why = match () {
        _ if !has => Some(NO_IMAGE),
        // The export's own area picking holds the canvas: no image crop meanwhile.
        _ if s.crop_pick && !s.crop_image => Some("Finish choosing the export area first"),
        _ => None,
    };
    let undo_why = (!s.can_undo()).then_some("Nothing to undo");
    let redo_why = (!s.can_redo()).then_some("Nothing to redo");
    col()
        .key("rail")
        .w(RAIL_W)
        .h_full()
        .min_h(0.0)
        .padding(8.0)
        .gap(2.0)
        .fill(t.panel)
        .child(rail_caption("File", true))
        .child(
            rail_button("open", glyph(&oc::FOLDER, t.muted), "Open…", "Ctrl+O", false, true).on_press(|s: &mut Editor| s.stitcher.open()),
        )
        .child(unavailable(
            rail_button("export", glyph(&oc::DOWNLOAD, t.muted), "Export…", "Ctrl+E", false, has)
                .on_press(|s: &mut Editor| s.export_open = true),
            no_image,
            true,
        ))
        .child(rail_caption("Tools", false))
        .child(tools)
        .child(unavailable(
            rail_button("crop-image", crop_icon(ink(s.crop_image)), "Crop", "C", s.crop_image, crop_why.is_none())
                .on_press(|s: &mut Editor| if s.crop_image { s.finish_crop(false) } else { s.begin_image_crop() }),
            crop_why,
            true,
        ))
        .child(unavailable(
            rail_button("add-legend", legend_icon(t.muted), "Legend", "G", false, has).on_press(|s: &mut Editor| s.add_legend()),
            no_image,
            true,
        ))
        .child(rail_caption("Edit", false))
        .child(unavailable(
            rail_button("undo", glyph(&oc::OUTLINE_RESET, t.muted), "Undo", "Ctrl+Z", false, undo_why.is_none())
                .on_press(|s: &mut Editor| s.undo()),
            undo_why,
            true,
        ))
        .child(unavailable(
            rail_button("redo", glyph(&oc::OUTLINE_ROTATE_CLOCKWISE, t.muted), "Redo", "Ctrl+Y", false, redo_why.is_none())
                .on_press(|s: &mut Editor| s.redo()),
            redo_why,
            true,
        ))
}

// ---- Canvas -----------------------------------------------------------------------------------

/// The selection's outline and handles, in viewport points.
fn selection_overlay(s: &Editor) -> Vec<El> {
    let Some(a) = s.selection() else { return Vec::new() };
    let accent = tk().accent;
    let [_, _, vw, vh] = s.viewport;
    let ids = s.picked();
    let several = ids.len() > 1;
    let mut out = Vec::new();
    // Each selected annotation's outline: boxes always, every kind when several are selected.
    for (n, id) in ids.iter().enumerate() {
        let Some(b) = s.get(*id) else { continue };
        if !several && !matches!(b.shape, Geo::Rect { .. } | Geo::Text { .. } | Geo::Dot { .. }) {
            continue;
        }
        let [x, y, w, h] = b.bounds();
        let p0 = s.to_screen([x, y]);
        let p1 = s.to_screen([x + w, y + h]);
        let (l, t, r, bt) = (p0[0] - 3.0, p0[1] - 3.0, p1[0] + 3.0, p1[1] + 3.0);
        let outline = Shape::new().stroke_with(vec![[l, t], [r, t], [r, bt], [l, bt]], true, 1.5, LineCap::Butt, accent);
        let key = if n == 0 { "sel-outline".to_string() } else { format!("sel-outline-{n}") };
        out.push(canvas(outline).key(key).absolute(0.0, 0.0).size(vw, vh).inert());
    }
    // Handles resize one annotation, with the select tool: not a group, not while moving.
    if several || s.tool == Tool::Move {
        return out;
    }
    for (i, h) in a.handles().into_iter().enumerate() {
        let q = s.to_screen(h);
        out.push(handle(format!("handle-{i}"), q, accent));
    }
    out
}

/// The selection rectangle being drawn with the select tool.
fn marquee_overlay(s: &Editor) -> Option<El> {
    let [x, y, w, h] = s.marquee?;
    let accent = tk().accent;
    let p0 = s.to_screen([x, y]);
    let p1 = s.to_screen([x + w, y + h]);
    Some(
        el().key("marquee")
            .absolute(p0[0], p0[1])
            .size((p1[0] - p0[0]).max(1.0), (p1[1] - p0[1]).max(1.0))
            .fill(accent.alpha(0.08))
            .border(1.0, accent)
            .border_dash(4.0, 3.0)
            .inert(),
    )
}

/// A square drag handle centred on `q`.
fn handle(key: String, q: [f32; 2], accent: Color) -> El {
    el().key(key).absolute(q[0] - 5.0, q[1] - 5.0).size(10.0, 10.0).radius(2.0).fill(Color::WHITE).border(1.5, accent).inert()
}

/// The area being chosen (to export, or to crop the image to): the rest of the image dimmed,
/// the area's outline and handles, its size, and a bar to finish.
fn crop_overlay(s: &Editor) -> Vec<El> {
    let t = tk();
    let [_, _, vw, vh] = s.viewport;
    let dim = Color::BLACK.alpha(0.6);
    let mut out = Vec::new();
    match s.crop_area() {
        None => out.push(el().key("crop-dim").absolute(0.0, 0.0).size(vw, vh).fill(dim).inert()),
        Some(r) => {
            let p0 = s.to_screen([r[0], r[1]]);
            let p1 = s.to_screen([r[0] + r[2], r[1] + r[3]]);
            let (l, top, rr, b) = (p0[0].clamp(0.0, vw), p0[1].clamp(0.0, vh), p1[0].clamp(0.0, vw), p1[1].clamp(0.0, vh));
            for (i, [x, y, w, h]) in
                [[0.0, 0.0, vw, top], [0.0, b, vw, vh - b], [0.0, top, l, b - top], [rr, top, vw - rr, b - top]].into_iter().enumerate()
            {
                out.push(el().key(format!("crop-dim-{i}")).absolute(x, y).size(w.max(0.0), h.max(0.0)).fill(dim).inert());
            }
            let outline = Shape::new().stroke_with(
                vec![[p0[0], p0[1]], [p1[0], p0[1]], [p1[0], p1[1]], [p0[0], p1[1]]],
                true,
                1.5,
                LineCap::Butt,
                Color::WHITE,
            );
            out.push(canvas(outline).key("crop-outline").absolute(0.0, 0.0).size(vw, vh).inert());
            for (i, h) in crop_handles(r).into_iter().enumerate() {
                out.push(handle(format!("crop-handle-{i}"), s.to_screen(h), t.accent));
            }
            let [_, _, cw, ch] = s.crop_area_px().unwrap_or_default();
            let label_y = if p0[1] > 34.0 { p0[1] - 30.0 } else { p0[1] + 8.0 };
            out.push(
                row()
                    .key("crop-size")
                    .absolute(p0[0].max(4.0), label_y)
                    .h(22.0)
                    .px(8.0)
                    .items_center()
                    .radius(6.0)
                    .fill(Color::BLACK.alpha(0.75))
                    .child(text(format!("{cw} × {ch} px")).font(TINY).color(Color::WHITE))
                    .inert(),
            );
        }
    }
    let (hint, done) = if s.crop_image {
        ("Drag on the image to choose the part to keep.", "Crop")
    } else {
        ("Drag on the image to choose the area to export.", "Use this area")
    };
    let bar_w = 580.0f32.min(vw - 24.0);
    let why = s.crop_area().is_none().then_some("Drag on the image to choose an area first");
    out.push(
        app::floating(
            row()
                .key("crop-bar")
                .absolute(((vw - bar_w) / 2.0).max(12.0), CROP_BAR_TOP)
                .w(bar_w)
                .h(CROP_BAR_H)
                .padding(7.0)
                .gap(8.0)
                .items_center(),
        )
        .child(el().grow(1.0).min_w(0.0).px(8.0).child(text(hint).font(UI).color(t.text).truncate()))
        .child(button("crop-cancel", "Cancel", Kind::Secondary, None, |s: &mut Editor| s.finish_crop(false)))
        .child(button("crop-done", done, Kind::Primary, why, |s: &mut Editor| s.finish_crop(true))),
    );
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
        // A selectable annotation under the select tool: a press picks it.
        Hover::Pick => Cursor::Pointer,
        Hover::ResizeX => Cursor::ResizeColumn,
        Hover::ResizeY => Cursor::ResizeRow,
        Hover::ResizeDown => cursors::resize_down(),
        Hover::ResizeUp => cursors::resize_up(),
        // Choosing an area, a press anywhere else draws a new one.
        Hover::Nothing if s.crop_pick => Cursor::Crosshair,
        Hover::Nothing => match s.tool {
            Tool::Select | Tool::Move => Cursor::Default,
            Tool::Draw(_) => Cursor::Crosshair,
        },
    }
}

/// What the canvas shows before an image is open (Basalt's empty state, with the main action).
fn empty_canvas() -> El {
    let s = tk();
    col()
        .gap(10.0)
        .items_center()
        .child(row().size(44.0, 44.0).center().radius(12.0).fill(s.tint).child(icon(&oc::OUTLINE_PHOTO, 20.0).color(s.muted)))
        .child(text("No image").font(TITLE).color(s.text))
        .child(text("Drop an image here, or open one (PNG, JPEG, WebP, BMP).").font(UI).color(s.muted))
        .child(row().pt(6.0).child(
            app::button("Open an image…", Kind::Primary, None, Some("Ctrl+O"), |s: &mut Editor| s.stitcher.open()).key("open-empty"),
        ))
}

fn viewport(s: &Editor) -> El {
    let t = tk();
    let mut v = el()
        .key("viewport")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .min_h(0.0)
        .clip_soft(0.0)
        .fill(t.bg)
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
        .on_secondary_press_local(|s: &mut Editor, e, _| s.open_context_menu([e.x, e.y]))
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
            let k = vector::text_boost(s.zoom);
            let world = el()
                .key("world")
                .absolute(0.0, 0.0)
                .size(iw, ih)
                .origin(0.0, 0.0)
                .live_offset(&s.live_pan)
                .live_scale(&s.live_zoom)
                .pixels(&s.base)
                .children(s.anns.iter().flat_map(|a| vector::annotation(a, iw, ih, &s.scale, k)))
                .inert();
            v = v.child(world);
            v = if s.crop_pick { v.children(crop_overlay(s)) } else { v.children(selection_overlay(s)).children(marquee_overlay(s)) };
            // Where the context menu opens.
            if let Some([x, y]) = s.context_menu {
                v = v.child(el().key("context-anchor").absolute(x, y).size(1.0, 1.0).inert());
            }
        }
        None => v = v.center().child(empty_canvas()),
    }
    v
}

// ---- Properties panel -------------------------------------------------------------------------

fn hatch_controls(st: &Style, custom: &[Rgba], unit: f32) -> El {
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
        .child(slider("hatch-alpha", "Opacity", percent(h.color.0[3]), 5.0, 100.0, pct(h.color.0[3]), |st, v| {
            hatch(st, |h| h.color = h.color.with_alpha(alpha(v)))
        }))
        .child(slider("hatch-spacing", "Spacing", h.spacing, 2.0, 80.0 * unit, px(h.spacing), |st, v| hatch(st, |h| h.spacing = v)))
        .child(slider("hatch-width", "Line width", h.width, 0.5, 20.0 * unit, px(h.width), |st, v| hatch(st, |h| h.width = v)))
        .child(slider("hatch-angle", "Angle", h.angle_deg, 0.0, 180.0, format!("{}°", h.angle_deg.round()), |st, v| {
            // Snap to 5° steps.
            hatch(st, |h| h.angle_deg = (v / 5.0).round() * 5.0)
        }));
    c
}

fn badge_shapes(current: BadgeShape) -> El {
    let items = BadgeShape::ALL.into_iter().map(move |(shape, name)| {
        let on = current == shape;
        // Drawn in the text colour: `segments` dims the unchosen ones until hovered.
        let ink = tk().text;
        let b = [3.0, 3.0, 16.0, 16.0];
        let drawing = match shape.polygon(b) {
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
        let press = move |s: &mut Editor| s.edit_style(|st| st.badge = shape);
        (format!("badge-{name}"), name.to_string(), Some(canvas(drawing).size(22.0, 22.0)), on, press)
    });
    segments(items, 36.0)
}

fn font_name(st: &Style) -> &str {
    st.font.as_deref().unwrap_or("Inter (default)")
}

/// The font picker: a search field and the matching installed families, each in its own face.
fn font_menu(s: &Editor, current: &Style) -> El {
    let t = tk();
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
            .radius(R - 2.0)
            .fill(if on { t.tint } else { Color::TRANSPARENT })
            .hover_fill(t.tint)
            .cursor(Cursor::Pointer)
            .child(text(label).font(style).color(t.text).truncate())
            .on_press(move |s: &mut Editor| {
                s.font_menu = false;
                s.edit_style(|st| st.font = family.clone());
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
        list = list.child(row().h(28.0).px(8.0).items_center().child(text("No installed font matches.").font(UI).color(t.muted)));
    }
    col()
        .key("font-menu")
        .w_full()
        .gap(6.0)
        .padding(6.0)
        .radius(R + 2.0)
        .fill(t.float)
        .border(1.0, t.hair)
        .child(field(
            row().w_full().gap(8.0).items_center().child(icon(&oc::MAGNIFYING_GLASS, 13.0).color(t.dim)).child(
                input(&s.font_query, "font-search", "Search fonts", "Search installed fonts…").on_edit(|s: &mut Editor| &mut s.font_query),
            ),
            false,
        ))
        .child(col().key("font-list").w_full().h(220.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
}

fn tabs(current: PanelTab, has_text: bool) -> El {
    let items = PanelTab::ALL
        .into_iter()
        .filter(move |(tab, _)| *tab != PanelTab::Text || has_text)
        .map(move |(tab, name)| (format!("tab-{name}"), name.to_string(), None, tab == current, move |s: &mut Editor| s.panel_tab = tab));
    segments(items, app::h()).key("tabs")
}

fn style_tab(s: &Editor, a: &Annotation, unit: f32) -> El {
    let st = &a.style;
    let custom = &s.palette[..];
    let boxy = matches!(a.kind, AnnKind::Rect | AnnKind::Hatch | AnnKind::Badge);
    let mut c = col().w_full().gap(10.0);
    if a.kind == AnnKind::Badge {
        c = c.child(section("Shape")).child(badge_shapes(st.badge));
    }
    if a.kind == AnnKind::Measure {
        c = c.child(scale_section(s, a));
    }
    if boxy {
        c = c.child(checkbox("show-size", "Show real size", st.show_size, |st| st.show_size = !st.show_size));
    }
    let stroke_title = if matches!(a.kind, AnnKind::Wire | AnnKind::Arrow | AnnKind::Measure) { "Line" } else { "Outline" };
    c = c
        .child(gap_section(stroke_title))
        .child(swatches("stroke", st.stroke, false, custom, Target::Stroke))
        .child(slider("stroke-width", "Width", st.stroke_width, 0.0, 40.0 * unit, px(st.stroke_width), |st, v| st.stroke_width = v))
        .child(slider("stroke-alpha", "Opacity", percent(st.stroke.0[3]), 5.0, 100.0, pct(st.stroke.0[3]), |st, v| {
            st.stroke = st.stroke.with_alpha(alpha(v))
        }));
    if !matches!(a.kind, AnnKind::Wire | AnnKind::Arrow) {
        let title = match a.kind {
            AnnKind::Text => "Background",
            AnnKind::Measure => "Label background",
            _ => "Fill",
        };
        c = c.child(gap_section(title)).child(swatches("fill", st.fill, true, custom, Target::Fill)).child(slider(
            "fill-alpha",
            "Opacity",
            percent(st.fill.0[3]),
            0.0,
            100.0,
            pct(st.fill.0[3]),
            |st, v| st.fill = st.fill.with_alpha(alpha(v)),
        ));
    }
    let rounded =
        matches!(a.kind, AnnKind::Rect | AnnKind::Hatch | AnnKind::Text) || (a.kind == AnnKind::Badge && st.badge == BadgeShape::Rounded);
    if rounded {
        c = c
            .child(gap_section("Corners"))
            .child(slider("radius", "Radius", st.radius, 0.0, 100.0 * unit, px(st.radius), |st, v| st.radius = v));
    }
    if boxy {
        c = c.child(gap_section("Hatching")).child(hatch_controls(st, custom, unit));
    }
    c
}

/// A section caption with room above it, after the first group of a tab.
fn gap_section(title: &str) -> El {
    row().w_full().pt(8.0).child(section(title))
}

fn text_tab(s: &Editor, a: &Annotation, unit: f32) -> El {
    let t = tk();
    let st = &a.style;
    let label = match a.kind {
        AnnKind::Text => "Text (Shift+Enter: new line)",
        AnnKind::Legend => "Title",
        AnnKind::Badge => "Number",
        _ => "Label",
    };
    let font_button = row()
        .key("font-button")
        .role("button")
        .label("Font")
        .w_full()
        .h(app::h())
        .px(10.0)
        .gap(8.0)
        .items_center()
        .radius(R)
        .fill(t.bg)
        .border(1.0, t.line)
        .hover_border(t.hair)
        .cursor(Cursor::Pointer)
        .focusable(true)
        .child(text(font_name(st)).font(font::text_style(st, 13.0)).color(t.text).truncate().grow(1.0))
        .child(icon(&oc::CHEVRON_DOWN, 12.0).color(t.dim).rotate(if s.font_menu { PI } else { 0.0 }))
        .on_press(|s: &mut Editor| s.font_menu = !s.font_menu);
    let mut c = col().w_full().gap(10.0);
    if a.kind != AnnKind::Measure {
        c = c.child(section(label)).child(field(
            input(&s.caption, "caption", "Annotation text", "Type some text…").on_edit(|s: &mut Editor| &mut s.caption),
            false,
        ));
    }
    if a.kind == AnnKind::Legend {
        c = c.child(legend_rows(s, a));
    }
    c = c.child(gap_section("Font")).child(font_button);
    if s.font_menu {
        c = c.child(font_menu(s, st));
    }
    c = c
        .child(checkbox("bold", "Bold", st.bold, |st| st.bold = !st.bold))
        .child(slider("font-size", "Size", st.font_size, 4.0, 200.0 * unit, px(st.font_size), |st, v| st.font_size = v))
        .child(gap_section("Colour"))
        .child(swatches("text-color", st.text_color, false, &s.palette, Target::Text));
    if matches!(a.kind, AnnKind::Text | AnnKind::Measure | AnnKind::Legend) || st.show_size {
        c = c.child(slider("padding", "Padding", st.padding, 0.0, 60.0 * unit, px(st.padding), |st, v| st.padding = v));
    }
    c
}

/// A legend's rows: a chip of each look and its description, editable in place.
fn legend_rows(s: &Editor, a: &Annotation) -> El {
    let t = tk();
    let mut c = col().w_full().gap(6.0).child(gap_section("Entries"));
    for (i, (item, edit)) in a.legend.iter().zip(&s.legend_edits).enumerate() {
        let chip = if matches!(item.kind, AnnKind::Badge | AnnKind::Dot) || item.style.fill.0[3] > 120 {
            item.style.fill
        } else {
            item.style.stroke
        };
        c = c.child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(el().size(14.0, 14.0).radius(4.0).fill(chip.with_alpha(255).to_rune()).border(1.0, t.hair))
                .child(
                    el().grow(1.0).min_w(0.0).child(field(
                        input(edit, format!("legend-{i}"), "Legend entry", "What this look means")
                            .on_edit(move |s: &mut Editor| &mut s.legend_edits[i]),
                        false,
                    )),
                )
                .child(
                    mini_button(format!("legend-remove-{i}"), &oc::CLOSE_SMALL, "Remove entry", 26.0, true)
                        .on_press(move |s: &mut Editor| s.remove_legend_row(i)),
                ),
        );
    }
    c.child(button("legend-update", "Update from annotations", Kind::Secondary, None, |s: &mut Editor| s.update_legend()))
}

fn arrange_tab() -> El {
    col()
        .w_full()
        .gap(8.0)
        .child(section("Stacking order"))
        .child(pair(
            button("front", "Bring to front", Kind::Secondary, None, |s: &mut Editor| s.restack(true, true)),
            button("back", "Send to back", Kind::Secondary, None, |s: &mut Editor| s.restack(false, true)),
        ))
        .child(pair(
            button("forward", "Forward", Kind::Secondary, None, |s: &mut Editor| s.restack(true, false)),
            button("backward", "Backward", Kind::Secondary, None, |s: &mut Editor| s.restack(false, false)),
        ))
        .child(gap_section("Annotation"))
        .child(pair(
            button("duplicate", "Duplicate", Kind::Secondary, None, |s: &mut Editor| s.duplicate_selected()),
            button("delete", "Delete", Kind::Danger, None, |s: &mut Editor| s.delete_selected()),
        ))
}

/// Calibration: the selected measure's real length sets the image's scale for every measure.
fn scale_section(s: &Editor, a: &Annotation) -> El {
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
        .child(note(status))
        .child(segmented("unit", &units, s.calib_unit, |s, u| s.set_unit(u)))
        .child(
            row()
                .w_full()
                .gap(6.0)
                .items_center()
                .child(
                    el().grow(1.0).min_w(0.0).child(field(
                        input(&s.calib_field, "calib-field", "Real length", "Real length, such as 4.5")
                            .on_edit(|s: &mut Editor| &mut s.calib_field)
                            .on_submit(|s: &mut Editor, _| s.calibrate()),
                        false,
                    )),
                )
                .child(button("calibrate", "Set scale", Kind::Primary, None, |s: &mut Editor| s.calibrate())),
        )
        .children(
            s.scale.mm_per_px.is_some().then(|| button("clear-scale", "Clear scale", Kind::Ghost, None, |s: &mut Editor| s.clear_scale())),
        )
}

fn export_quality(o: &ExportOptions) -> El {
    let label = if o.format == Format::Pdf { "Photo quality" } else { "Quality" };
    let frac = (o.quality as f32 - 1.0) / 99.0;
    slider_track("export-quality", label, frac, o.quality.to_string(), EXPORT_W, |s, v, _| {
        s.export.quality = (1.0 + v * 99.0).round() as u8;
    })
}

fn export_view(s: &Editor) -> El {
    let t = tk();
    let o = &s.export;
    let (iw, ih) = s.picture.as_ref().map_or((1, 1), |p| p.rgba.dimensions());
    let [_, _, rw, rh] = s.export_rect().unwrap_or([0, 0, iw, ih]);
    let (w, h) = o.size((rw, rh));
    // Trimming only means something for an image with transparent edges (a mosaic).
    let has_edges = s.picture.as_ref().is_some_and(|p| p.filled != [0, 0, iw, ih]);
    let mut crops = vec![(Area::Whole, "None".to_string())];
    if has_edges {
        crops.push((Area::Filled, "Trim edges".to_string()));
    }
    crops.push((Area::Custom, "Select area".to_string()));
    let crop = if o.area == Area::Filled && !has_edges { Area::Whole } else { o.area };
    let formats: Vec<_> = Format::ALL.iter().map(|f| (*f, f.name().to_string())).collect();
    let scales: Vec<_> = [0.25f32, 0.5, 1.0, 2.0].iter().map(|k| (*k, format!("{} %", (k * 100.0).round()))).collect();
    let result = if o.format == Format::Pdf {
        format!("{} × {} pt page", (w as f32 * 0.75).round(), (h as f32 * 0.75).round())
    } else {
        format!("{w} × {h} px")
    };
    let format_note = match o.format {
        Format::Png => "Lossless, with transparency.",
        Format::Jpeg => "Smallest for photos, no transparency.",
        Format::Webp => "Lossless, smaller than PNG, with transparency.",
        Format::Pdf => "Vector annotations with selectable text.",
    };
    let mut card = dialog_card("export-dialog", EXPORT_W, "Export")
        .child(section("Format"))
        .child(segmented("export-format", &formats, o.format, |s, f| s.export.format = f))
        .child(note(format_note))
        .child(gap_section("Crop"))
        .child(segmented("export-area", &crops, crop, |s, a| s.set_export_area(a)))
        .children((crop == Area::Custom).then(|| {
            row()
                .w_full()
                .gap(8.0)
                .items_center()
                .child(text(format!("Area of {rw} × {rh} px")).font(TINY).color(t.muted))
                .child(spacer())
                .child(button("export-area-edit", "Redraw…", Kind::Ghost, None, |s: &mut Editor| s.begin_crop()))
        }))
        .child(gap_section("Size"))
        .child(segmented("export-scale", &scales, o.scale, |s, k| s.export.scale = k));
    if o.format == Format::Jpeg || (o.format == Format::Pdf && o.photo) {
        card = card.child(export_quality(o));
    }
    if o.format.can_drop_photo() {
        card =
            card.child(app::checkbox("Include the photo", o.photo, |s: &mut Editor| s.export.photo = !s.export.photo).key("export-photo"));
    }
    if o.format == Format::Png {
        card = card.child(
            app::checkbox("Smaller file (slower)", o.small_png, |s: &mut Editor| s.export.small_png = !s.export.small_png)
                .key("export-small"),
        );
    }
    card = card.child(dialog_actions(
        Some(text(result).font(TINY).color(t.muted)),
        vec![
            button("export-cancel", "Cancel", Kind::Secondary, None, |s: &mut Editor| s.export_open = false),
            button_tx("export-go", "Export…", Kind::Primary, export_dialog),
        ],
    ));
    dialog_scrim("export-scrim", card, |s| s.export_open = false)
}

/// A dashed area of the open menu that takes dropped files, lit while a drag is over it, with
/// the buttons that pick files the other way.
fn drop_zone(over: bool, title: &str, buttons: El) -> El {
    let t = tk();
    col()
        .key("drop-zone")
        .w_full()
        .padding(18.0)
        .gap(12.0)
        .items_center()
        .radius(R + 2.0)
        .fill(if over { t.accent.alpha(0.12) } else { t.bg })
        .border(1.0, if over { t.accent } else { t.dim })
        .border_dash(6.0, 4.0)
        .child(icon(&oc::DOWNLOAD, 18.0).color(if over { t.text } else { t.muted }))
        .child(text(title).font(UI_STRONG).color(if over { t.text } else { t.muted }))
        .child(buttons)
        .on_drop_files(|s: &mut Editor, files, _| drop_files(s, files))
        .accepts_extensions(&IMAGE_EXTENSIONS)
        .on_drop_hover(|s: &mut Editor, over, _| s.stitcher.drop_over = over)
}

fn stitch_view(s: &Editor) -> El {
    let t = tk();
    let st = &s.stitcher;
    let title = if st.mode == Mode::Stitch && st.stage != Stage::Setup { "Stitch images" } else { "Open" };
    let mut card = dialog_card("stitch-dialog", STITCH_W, title);
    match &st.stage {
        Stage::Setup => {
            let modes = [(Mode::Single, "One image".to_string()), (Mode::Stitch, "Stitch several".to_string())];
            card = card.child(segmented("open-mode", &modes, st.mode, |s, m| s.stitcher.mode = m));
            if st.mode == Mode::Single {
                card = card
                    .child(note("Open one image to annotate."))
                    .child(drop_zone(
                        st.drop_over,
                        "Drop an image here",
                        row().gap(8.0).child(button_tx("open-choose", "Choose an image…", Kind::Primary, choose_image)),
                    ))
                    .child(dialog_actions(
                        None,
                        vec![button("stitch-cancel", "Cancel", Kind::Secondary, None, |s: &mut Editor| s.stitcher.close())],
                    ));
            } else {
                let shown = 8;
                let mut list = col().w_full().gap(2.0);
                for (i, p) in st.paths.iter().take(shown).enumerate() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    list = list.child(
                        row()
                            .w_full()
                            .h(28.0)
                            .pl(10.0)
                            .pr(2.0)
                            .gap(8.0)
                            .items_center()
                            .radius(R - 2.0)
                            .fill(t.tint)
                            .child(icon(&oc::OUTLINE_PHOTO, 13.0).color(t.dim))
                            .child(el().grow(1.0).min_w(0.0).child(text(name).font(UI).color(t.text).truncate()))
                            .child(
                                mini_button(format!("stitch-rm-{i}"), &oc::CLOSE_SMALL, "Remove", 24.0, true)
                                    .on_press(move |s: &mut Editor| s.stitcher.remove(i)),
                            ),
                    );
                }
                let hidden = st.paths.len().saturating_sub(shown);
                if hidden > 0 {
                    list = list.child(text(format!("and {hidden} more")).font(TINY).color(t.muted));
                }
                let summary = match st.paths.len() {
                    0 => "No images yet: add two or more.".to_string(),
                    1 => "1 image. Add at least one more.".to_string(),
                    n => format!("{n} images"),
                };
                let output_note = match st.output {
                    Output::Editor => "Opens in the editor, up to 8192 px a side, ready to annotate.",
                    Output::File => "As large as the photos allow (up to 200 megapixels), saved as PNG or WebP.",
                };
                let outputs = [(Output::Editor, "Open in Layr".to_string()), (Output::File, "Save to a file".to_string())];
                card = card
                    .child(note("Photos are joined by what they have in common, in any order, turned or taken at an angle. Each should share about a third of its view with another."))
                    .child(gap_section("Images"))
                    .child(drop_zone(
                        st.drop_over,
                        "Drop photos here",
                        row()
                            .gap(8.0)
                            .child(button(
                                "stitch-add",
                                "Add images…",
                                Kind::Secondary,
                                st.picking().then_some("Already choosing images"),
                                |s: &mut Editor| s.stitcher.pick_images(),
                            ))
                            .child(button_tx("stitch-folder", "Add a folder…", Kind::Secondary, add_folder)),
                    ))
                    .child(
                        row()
                            .w_full()
                            .items_center()
                            .child(text(summary).font(TINY).color(t.muted))
                            .child(spacer())
                            .child(button(
                                "stitch-clear",
                                "Clear",
                                Kind::Ghost,
                                st.paths.is_empty().then_some("No images to clear"),
                                |s: &mut Editor| s.stitcher.paths.clear(),
                            )),
                    )
                    .child(list)
                    .child(gap_section("Result"))
                    .child(segmented("stitch-output", &outputs, st.output, |s, o| s.stitcher.output = o))
                    .child(note(output_note))
                    .child(app::checkbox("Crop to a rectangle", st.crop, |s: &mut Editor| s.stitcher.crop = !s.stitcher.crop).key("stitch-crop"))
                    .child(note(if st.crop {
                        "Keeps the largest rectangle the photos fill, without the ragged edges of the assembly."
                    } else {
                        "Keeps everything; the area no photo covers stays transparent."
                    }))
                    .child(dialog_actions(
                        None,
                        vec![
                            button("stitch-cancel", "Cancel", Kind::Secondary, None, |s: &mut Editor| s.stitcher.close()),
                            button(
                                "stitch-go",
                                "Stitch",
                                Kind::Primary,
                                (!st.can_start()).then_some("Add at least two images to stitch"),
                                |s: &mut Editor| s.stitcher.start(),
                            ),
                        ],
                    ));
            }
        }
        Stage::Running => {
            let (stage, frac) = st.status().unwrap_or_default();
            let label = if stage.is_empty() { "Starting…".to_string() } else { format!("{stage}…") };
            card = card.child(app::progress(&label, frac).key("stitch-bar")).child(dialog_actions(
                Some(app::spinner(14.0)),
                vec![button("stitch-stop", "Cancel", Kind::Secondary, None, |s: &mut Editor| s.stitcher.close())],
            ));
        }
        Stage::Failed(e) => {
            card = card
                .child(
                    row()
                        .w_full()
                        .gap(8.0)
                        .items_start()
                        .child(row().pt(6.0).child(app::dot(Level::Err)))
                        .child(el().grow(1.0).min_w(0.0).child(text(e.clone()).font(UI).color(t.text).wrap())),
                )
                .child(dialog_actions(
                    None,
                    vec![button("stitch-close", "Close", Kind::Primary, None, |s: &mut Editor| s.stitcher.close())],
                ));
        }
        Stage::Idle => {}
    }
    // While a modal is open only it takes drops, so the menu itself does.
    let card = card.on_drop_files(|s: &mut Editor, files, _| drop_files(s, files)).accepts_extensions(&IMAGE_EXTENSIONS);
    dialog_scrim("stitch-scrim", card, |s| s.stitcher.close())
}

/// The properties panel of the selected annotation: its kind, tabs, and the tab's controls.
fn properties(s: &Editor, a: &Annotation) -> El {
    let t = tk();
    let unit = s.picture.as_ref().map_or(1.0, |p| p.unit());
    let has_text = matches!(a.kind, AnnKind::Rect | AnnKind::Hatch | AnnKind::Badge | AnnKind::Text | AnnKind::Measure | AnnKind::Legend);
    let tab = if s.panel_tab == PanelTab::Text && !has_text { PanelTab::Style } else { s.panel_tab };
    let body = match tab {
        PanelTab::Style => style_tab(s, a, unit),
        PanelTab::Text => text_tab(s, a, unit),
        PanelTab::Arrange => arrange_tab(),
    };
    // Several selected: the panel shows the first one's controls and edits them all.
    let title = match s.picked().len() {
        n if n > 1 => format!("{n} annotations"),
        _ => a.kind.name().to_string(),
    };
    col()
        .key("panel")
        .w(PANEL_W)
        .h_full()
        .min_h(0.0)
        .fill(t.panel)
        .child(app::pane_header(&title, el()))
        .child(row().w_full().px(PANEL_PAD).pt(12.0).pb(2.0).child(tabs(tab, has_text)))
        .child(
            el().w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(app::scroll_view("panel-scroll", col().w_full().padding(PANEL_PAD).pb(24.0).child(body))),
        )
}

// ---- Colour editor ----------------------------------------------------------------------------

/// A draggable bar picking one of the editor's channels from 0 to 1.
fn channel(key: &'static str, label: &str, value: f32, stops: Vec<(f32, Color)>, set: fn(&mut ColorTool, f32)) -> El {
    let t = tk();
    let knob_x = value.clamp(0.0, 1.0) * PICKER_W - 7.0;
    col().w_full().gap(5.0).child(text(label).font(TINY).color(t.muted)).child(
        el().key(key)
            .role("slider")
            .label(label)
            .w(PICKER_W)
            .h(18.0)
            .radius(9.0)
            .linear_gradient(FRAC_PI_2, stops)
            .border(1.0, t.hair)
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

fn color_editor(s: &Editor, tool: &ColorTool) -> El {
    let t = tk();
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

    let card = dialog_card("color-editor", PICKER_W, tool.target.name())
        .child(
            row().w_full().gap(12.0).items_center().child(el().size(48.0, 48.0).radius(10.0).fill(c.to_rune()).border(1.0, t.hair)).child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(6.0)
                    .child(field(
                        input(&tool.field, "color-field", "Colour", "#RRGGBB, rgb(), hsl() or a name")
                            .on_edit(|s: &mut Editor| &mut s.color_tool.as_mut().expect("colour editor is open").field),
                        tool.invalid,
                    ))
                    .children(tool.invalid.then(|| {
                        row().w_full().gap(6.0).items_start().child(row().pt(5.0).child(app::dot(Level::Err))).child(
                            el().grow(1.0).min_w(0.0).child(
                                text("Not a colour: try #3FA7D6, rgb(63 167 214), hsl(200 65% 54%) or tomato.")
                                    .font(TINY)
                                    .color(t.danger)
                                    .wrap(),
                            ),
                        )
                    })),
            ),
        )
        .child(channel("hue", "Hue", tool.h / 360.0, hue, |t, v| t.h = v * 360.0))
        .child(channel("saturation", "Saturation", tool.s, sat, |t, v| t.s = v))
        .child(channel("lightness", "Lightness", tool.l, light, |t, v| t.l = v))
        .child(channel("alpha", "Opacity", tool.a, alpha, |t, v| t.a = v))
        .child(gap_section("Palette"))
        .child(grid(picks, 11))
        .child(pair(
            button("random-flat", "Random flat colour", Kind::Secondary, None, |s: &mut Editor| {
                if let Some(tool) = &mut s.color_tool {
                    let a = tool.color().0[3];
                    if let Some(c) = color::random_flat(1).first() {
                        tool.set_color(c.with_alpha(a));
                    }
                }
            }),
            button("random-palette", "Add 8 to palette", Kind::Secondary, None, |s: &mut Editor| s.add_random_colors(8)),
        ))
        .child(pair(
            button("save-color", "Save to palette", Kind::Secondary, None, |s: &mut Editor| {
                if let Some(c) = s.color_tool.as_ref().map(ColorTool::color) {
                    s.add_to_palette(c);
                }
            }),
            button(
                "clear-palette",
                "Clear my colours",
                Kind::Secondary,
                s.palette.is_empty().then_some("No saved colours to clear"),
                |s: &mut Editor| s.clear_palette(),
            ),
        ))
        .child(dialog_actions(
            None,
            vec![
                button("color-cancel", "Cancel", Kind::Secondary, None, |s: &mut Editor| s.color_tool = None),
                button("color-apply", "Apply", Kind::Primary, None, |s: &mut Editor| s.apply_color_tool()),
            ],
        ));
    dialog_scrim("color-scrim", card, |s| s.color_tool = None)
}

// ---- Updates ----------------------------------------------------------------------------------

/// A bar above the status line when a newer release exists, with what can be done about it.
fn update_banner(s: &Editor) -> Option<El> {
    let t = tk();
    let open_page = |s: &mut Editor| {
        if let Some(url) = s.updater.page() {
            update::open_url(&url);
        }
    };
    let release = button("update-page", "Release page", Kind::Ghost, None, open_page);
    let close = button("update-close", "Dismiss", Kind::Ghost, None, |s: &mut Editor| s.updater.dismiss());
    let (lead, message, buttons): (El, String, Vec<El>) = match &s.updater.state {
        State::Available(o) if o.can_install() => (
            app::dot(Level::Ok),
            format!("Layr {} is available (you have {}).", o.release.version, update::VERSION),
            vec![button("update-install", "Update now", Kind::Primary, None, |s: &mut Editor| s.updater.install()), release, close],
        ),
        State::Available(o) => (
            app::dot(Level::Ok),
            format!("Layr {} is available (you have {}). Download it from its release page.", o.release.version, update::VERSION),
            vec![button("update-page", "Open release page", Kind::Primary, None, open_page), close],
        ),
        State::Downloading { offer, done, total } => {
            let pct = if *total > 0 { format!(" {} %", (done * 100 / total).min(100)) } else { String::new() };
            (app::spinner(12.0), format!("Downloading Layr {}…{pct}", offer.release.version), Vec::new())
        }
        State::Installing(offer) => {
            (app::spinner(12.0), format!("Installing Layr {}… the app restarts by itself.", offer.release.version), Vec::new())
        }
        State::Failed { offer, message } => (
            app::dot(Level::Err),
            format!("Couldn't update to Layr {}: {message}", offer.release.version),
            vec![button("update-page", "Open release page", Kind::Primary, None, open_page), close],
        ),
        State::Idle | State::Checking => return None,
    };
    Some(
        row()
            .key("update-banner")
            .w_full()
            .h(44.0)
            .px(12.0)
            .gap(10.0)
            .items_center()
            .fill(t.float)
            .border_sides([1.0, 0.0, 0.0, 0.0], t.line)
            .child(lead)
            .child(el().grow(1.0).min_w(0.0).child(text(message).font(UI).color(t.text).truncate()))
            .children(buttons),
    )
}

// ---- Context menu -----------------------------------------------------------------------------

/// The canvas's context menu, open where it was right-clicked: the clipboard and the
/// selection's actions. Returns the layers to mount over the window.
fn context_menu(s: &Editor) -> Vec<El> {
    let Some(q) = s.context_menu else { return Vec::new() };
    let has = s.selection().is_some();
    let item = |label: &str, keys: &str, on: bool| {
        let it = app::MenuItem::new(label).shortcut(&shortcut_text(keys));
        Some(if on { it } else { it.disabled() })
    };
    let items = vec![
        item("Cut", "Ctrl+X", has),
        item("Copy", "Ctrl+C", has),
        item("Paste", "Ctrl+V", s.can_paste()),
        item("Duplicate", "Ctrl+D", has),
        None,
        item("Bring to front", "Ctrl+]", has),
        item("Send to back", "Ctrl+[", has),
        None,
        item("Delete", "Del", has).map(app::MenuItem::danger),
    ];
    // A paste lands where the menu was opened.
    let at = s.to_image(q);
    let pick = move |s: &mut Editor, i: usize| {
        s.context_menu = None;
        match i {
            0 => s.cut(),
            1 => s.copy(),
            2 => s.paste(Some(at)),
            3 => s.duplicate_selected(),
            5 => s.restack(true, true),
            6 => s.restack(false, true),
            8 => s.delete_selected(),
            _ => {}
        }
    };
    vec![
        rune_kit::scrim("context-scrim", |s: &mut Editor| s.context_menu = None),
        app::menu_panel(&items, None, pick).key("context-menu").anchored("context-anchor", Side::Bottom, Align::Start, 2.0),
    ]
}

// ---- Window -----------------------------------------------------------------------------------

/// The status bar: the last message (with the image's size), zoom and fit, and the version (which
/// checks for updates).
fn status_bar(s: &Editor) -> El {
    let t = tk();
    let has = s.picture.is_some();
    let no_image = (!has).then_some(NO_IMAGE);
    let zoom_out = mini_button("zoom-out", &oc::MINUS, "Zoom out", 20.0, has).on_press(|s: &mut Editor| s.zoom_center(1.0 / 1.25));
    let zoom_in = mini_button("zoom-in", &oc::PLUS, "Zoom in", 20.0, has).on_press(|s: &mut Editor| s.zoom_center(1.25));
    let fit = usable(
        row()
            .key("fit")
            .role("button")
            .label("Fit to window")
            .h(20.0)
            .px(6.0)
            .gap(5.0)
            .items_center()
            .radius(R - 3.0)
            .child(icon(&oc::OUTLINE_MAXIMIZE, 11.0).color(t.muted))
            .child(text("Fit").font(TINY).color(t.muted)),
        has,
    )
    .on_press(|s: &mut Editor| s.fit());
    row()
        .key("status")
        .w_full()
        .h(28.0)
        .px(12.0)
        .gap(12.0)
        .items_center()
        .fill(t.bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], t.line)
        .child(el().grow(1.0).min_w(0.0).child(text(&s.status).font(TINY).color(t.muted).truncate()))
        .child(
            row()
                .gap(2.0)
                .items_center()
                .child(unavailable(zoom_out, no_image, false))
                .child(row().w(46.0).justify_center().child(text(format!("{:.0} %", s.zoom * 100.0)).key("zoom").font(TINY).color(t.muted)))
                .child(unavailable(zoom_in, no_image, false))
                .child(unavailable(fit, no_image, false)),
        )
        .child(el().w(1.0).h(14.0).fill(t.line))
        .child(
            row()
                .key("version")
                .role("button")
                .label("Check for updates")
                .h(20.0)
                .px(6.0)
                .items_center()
                .radius(R - 3.0)
                .hover_fill(t.tint)
                .cursor(Cursor::Pointer)
                .help("Check for updates")
                .child(text(format!("Layr {}", update::VERSION)).font(CAPTION).color(t.dim))
                .on_press(|s: &mut Editor| s.updater.check(true)),
        )
}

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
    // Basalt's dark theme, its accent (selection, handles, drop targets) the system's.
    rune_basalt::set_theme(Theme::dark().accent(cx.accent()));
    // Panels, toggles and dialogs change in place, without springs or slides.
    cx.reduced_motion(true);
    let t = tk();
    let sep = || el().w(1.0).h_full().fill(t.line);
    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .fill(t.panel)
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
                .child(rail(s))
                .child(sep())
                .child(viewport(s))
                .children(s.selection().map(|a| [sep(), properties(s, a)]).into_iter().flatten()),
        )
        .children(update_banner(s))
        .child(status_bar(s));
    if let Some(tool) = &s.color_tool {
        root = root.child(color_editor(s, tool));
    }
    if s.export_open && s.picture.is_some() {
        root = root.child(export_view(s));
    }
    if s.stitcher.is_open() {
        root = root.child(stitch_view(s));
    }
    rune_kit::root(root, context_menu(s))
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
        Key::Escape if s.context_menu.is_some() => s.context_menu = None,
        Key::Escape if s.color_tool.is_some() => s.color_tool = None,
        Key::Enter if s.color_tool.is_some() => s.apply_color_tool(),
        Key::Escape if s.font_menu => s.font_menu = false,
        Key::Escape if s.crop_pick => s.finish_crop(false),
        Key::Enter if s.crop_pick => s.finish_crop(true),
        Key::Escape if s.stitcher.is_open() => s.stitcher.close(),
        Key::Enter if s.stitcher.can_start() => s.stitcher.start(),
        Key::Escape if s.export_open => s.export_open = false,
        Key::Enter if s.export_open => export_dialog(s, tx),
        Key::Command(c) => match c {
            'z' if shift => s.redo(),
            'z' => s.undo(),
            'y' => s.redo(),
            'o' => s.stitcher.open(),
            'e' | 's' if s.picture.is_some() && !s.crop_pick => s.export_open = true,
            'c' => s.copy(),
            // In a text field, Ctrl+X cuts its text, not the annotation.
            'x' if !typing => s.cut(),
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
        _ if typing || s.color_tool.is_some() || s.export_open || s.stitcher.is_open() || s.crop_pick => return false,
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
            "c" | "C" if s.picture.is_some() => s.begin_image_crop(),
            other => match Tool::from_shortcut(other) {
                Some(tool) if s.picture.is_some() => s.set_tool(tool),
                _ => return false,
            },
        },
        _ => return false,
    }
    true
}
