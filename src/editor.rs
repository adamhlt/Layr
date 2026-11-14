//! Editor state: the loaded image, the annotations, the camera, tools, selection and history.

use std::path::{Path, PathBuf};

use rune::{Drag, Live, Phase, Pixels, PointerButton, TextEdit, Tx};

use crate::color::{self, ColorTool};

use crate::measure::{Scale, Unit};
use crate::model::{Annotation, Handle, Kind, Shape, Style, snap45};
use crate::render::{self, ExportOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Draw(Kind),
}

impl Tool {
    pub const ALL: [(Tool, &'static str, &'static str); 9] = [
        (Tool::Select, "Select", "V"),
        (Tool::Draw(Kind::Rect), "Rectangle", "R"),
        (Tool::Draw(Kind::Hatch), "Hatch", "H"),
        (Tool::Draw(Kind::Badge), "Badge", "N"),
        (Tool::Draw(Kind::Wire), "Wire", "L"),
        (Tool::Draw(Kind::Arrow), "Arrow", "A"),
        (Tool::Draw(Kind::Dot), "Dot", "P"),
        (Tool::Draw(Kind::Text), "Text", "T"),
        (Tool::Draw(Kind::Measure), "Measure", "M"),
    ];

    pub fn from_shortcut(c: &str) -> Option<Tool> {
        Tool::ALL.iter().find(|(_, _, k)| k.eq_ignore_ascii_case(c)).map(|(t, _, _)| *t)
    }
}

pub struct Picture {
    pub path: PathBuf,
    pub rgba: image::RgbaImage,
}

impl Picture {
    pub fn size(&self) -> (f32, f32) {
        (self.rgba.width() as f32, self.rgba.height() as f32)
    }
    /// Default lengths scale with the photo: a 2000 px image is the reference.
    pub fn unit(&self) -> f32 {
        let (w, h) = self.size();
        (w.max(h) / 2000.0).clamp(0.25, 6.0)
    }
}

#[derive(Clone, Debug)]
enum Gesture {
    None,
    Pan { start: [f32; 2] },
    Move { orig: Annotation, start: [f32; 2], changed: bool },
    Handle { orig: Annotation, handle: Handle, changed: bool },
    Create { start: [f32; 2] },
}

/// The properties panel's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Style,
    Text,
    Arrange,
}

impl PanelTab {
    pub const ALL: [(PanelTab, &'static str); 3] = [(PanelTab::Style, "Style"), (PanelTab::Text, "Text"), (PanelTab::Arrange, "Arrange")];
}

/// What the pointer would do on the canvas, for its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hover {
    Nothing,
    /// Over an annotation the select tool would move.
    Body,
    /// Over a handle that resizes along x, y or a diagonal (`\\` or `/`).
    ResizeX,
    ResizeY,
    ResizeDown,
    ResizeUp,
    /// Over a point handle (a wire vertex, an arrow end).
    Point,
}

/// Handle grab radius, in screen points.
const GRAB: f32 = 8.0;

pub struct Editor {
    pub picture: Option<Picture>,
    pub anns: Vec<Annotation>,
    next_id: u64,
    pub selected: Option<u64>,
    pub tool: Tool,
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,

    pub base: Pixels,
    dirty: bool,

    pub pan: [f32; 2],
    pub zoom: f32,
    pub live_pan: Live<[f32; 2]>,
    pub live_zoom: Live<f32>,
    /// The canvas viewport, `[x, y, w, h]` in window points.
    pub viewport: [f32; 4],
    fit_pending: bool,

    gesture: Gesture,
    /// The wire being drawn: its last point follows the pointer.
    pub drawing_wire: Option<u64>,
    pub space: bool,
    pub hover: Hover,
    /// A right-button pan: where the pointer and the camera were when it began.
    right_pan: Option<([f32; 2], [f32; 2])>,

    /// The text field editing the selected caption or label.
    pub caption: TextEdit,
    caption_for: Option<u64>,
    caption_pushed: bool,
    /// Focus the caption field (with its text selected) once it shows.
    focus_caption: bool,

    /// The colour editor, open on one of the selection's colours.
    pub color_tool: Option<ColorTool>,
    /// Colours the user made, after the built-in ones; saved between runs.
    pub palette: Vec<crate::model::Rgba>,
    pub font_menu: bool,
    /// The text fields editing the selected legend's rows.
    pub legend_edits: Vec<TextEdit>,
    legend_for: Option<u64>,
    legend_pushed: bool,
    pub panel_tab: PanelTab,

    /// Copied or cut annotations, and how many times they've been pasted.
    clipboard: Vec<Annotation>,
    pastes: u32,
    /// The pointer over the canvas, in image pixels: where a paste lands.
    pointer: Option<[f32; 2]>,

    /// Image pixels to real lengths.
    pub scale: Scale,
    /// The real length typed to calibrate the scale.
    pub calib_field: TextEdit,
    pub calib_unit: Unit,

    /// The export dialog, open, and the settings it remembers.
    pub export_open: bool,
    pub export: ExportOptions,
    pub font_query: TextEdit,

    pub status: String,
}

impl Editor {
    pub fn new() -> Editor {
        Editor {
            picture: None,
            anns: Vec::new(),
            next_id: 1,
            selected: None,
            tool: Tool::Select,
            undo: Vec::new(),
            redo: Vec::new(),
            base: Pixels::new(),
            dirty: false,
            pan: [0.0, 0.0],
            zoom: 1.0,
            live_pan: Live::new([0.0, 0.0]),
            live_zoom: Live::new(1.0),
            viewport: [0.0, 0.0, 0.0, 0.0],
            fit_pending: false,
            gesture: Gesture::None,
            drawing_wire: None,
            space: false,
            hover: Hover::Nothing,
            right_pan: None,
            caption: TextEdit::new(""),
            caption_for: None,
            caption_pushed: false,
            focus_caption: false,
            color_tool: None,
            palette: color::load_palette(),
            font_menu: false,
            legend_edits: Vec::new(),
            legend_for: None,
            legend_pushed: false,
            panel_tab: PanelTab::Style,
            clipboard: Vec::new(),
            pastes: 0,
            pointer: None,
            scale: Scale::default(),
            calib_field: TextEdit::new(""),
            calib_unit: Unit::Millimetre,
            export_open: false,
            export: ExportOptions::default(),
            font_query: TextEdit::new(""),
            status: "Open an image (Ctrl+O) or drop one on the window.".into(),
        }
    }

    // ---- Image ------------------------------------------------------------------------------

    pub fn load(&mut self, path: &Path) {
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                if w > 8192 || h > 8192 {
                    self.status = format!("Image too large ({w}×{h}): 8192 px per side at most.");
                    return;
                }
                self.base.set(w, h, rgba.as_raw().clone());
                self.picture = Some(Picture { path: path.to_path_buf(), rgba });
                self.anns.clear();
                self.undo.clear();
                self.redo.clear();
                self.selected = None;
                self.drawing_wire = None;
                self.next_id = 1;
                self.dirty = true;
                self.fit_pending = true;
                self.fit();
                self.status = format!("{} — {w}×{h} px", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }
            Err(e) => self.status = format!("Couldn't open {}: {e}", path.display()),
        }
    }

    pub fn export(&mut self, path: &Path) {
        self.finish_wire();
        let Some(pic) = &self.picture else { return };
        self.status = match render::export(&pic.rgba, &self.anns, &self.scale, &self.export, path) {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    pub fn export_name(&self) -> String {
        let stem = self
            .picture
            .as_ref()
            .and_then(|p| p.path.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "image".into());
        format!("{stem}-annotated.{}", self.export.format.extensions()[0])
    }

    /// Called before every frame: writes typing back into the selection. True when the
    /// annotations changed outside a handler, so the view needs to run again.
    pub fn refresh(&mut self) -> bool {
        let caption = self.sync_caption() | self.sync_legend();
        let color = self.color_tool.as_mut().is_some_and(ColorTool::sync_hex);
        std::mem::take(&mut self.dirty) || caption || color
    }

    // ---- Camera -----------------------------------------------------------------------------

    fn set_camera(&mut self, pan: [f32; 2], zoom: f32) {
        self.zoom = zoom.clamp(0.02, 40.0);
        self.pan = pan;
        self.live_pan.set(self.pan);
        self.live_zoom.set(self.zoom);
    }

    pub fn set_viewport(&mut self, r: [f32; 4]) {
        self.viewport = r;
        if self.fit_pending {
            self.fit();
        }
    }

    /// Fits the whole image in the viewport.
    pub fn fit(&mut self) {
        let Some(pic) = &self.picture else { return };
        let [_, _, vw, vh] = self.viewport;
        if vw < 1.0 || vh < 1.0 {
            return;
        }
        self.fit_pending = false;
        let (iw, ih) = pic.size();
        let zoom = (vw / iw).min(vh / ih) * 0.96;
        self.set_camera([(vw - iw * zoom) / 2.0, (vh - ih * zoom) / 2.0], zoom);
    }

    /// Zooms by `factor` keeping viewport point `q` still.
    pub fn zoom_about(&mut self, q: [f32; 2], factor: f32) {
        let p = self.to_image(q);
        let zoom = (self.zoom * factor).clamp(0.02, 40.0);
        self.set_camera([q[0] - p[0] * zoom, q[1] - p[1] * zoom], zoom);
    }

    pub fn zoom_center(&mut self, factor: f32) {
        let [_, _, vw, vh] = self.viewport;
        self.zoom_about([vw / 2.0, vh / 2.0], factor);
    }

    pub fn to_image(&self, q: [f32; 2]) -> [f32; 2] {
        [(q[0] - self.pan[0]) / self.zoom, (q[1] - self.pan[1]) / self.zoom]
    }

    pub fn to_screen(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.zoom + self.pan[0], p[1] * self.zoom + self.pan[1]]
    }

    // ---- History ----------------------------------------------------------------------------

    pub fn push_history(&mut self) {
        self.undo.push(self.anns.clone());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn drop_last_history(&mut self) {
        self.undo.pop();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.finish_wire();
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.anns, prev));
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        self.finish_wire();
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.anns, next));
            self.after_history();
        }
    }

    fn after_history(&mut self) {
        if self.selected.is_some_and(|id| self.get(id).is_none()) {
            self.selected = None;
        }
        self.caption_for = None;
        self.dirty = true;
    }

    // ---- Annotations ------------------------------------------------------------------------

    pub fn get(&self, id: u64) -> Option<&Annotation> {
        self.anns.iter().find(|a| a.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Annotation> {
        self.anns.iter_mut().find(|a| a.id == id)
    }

    pub fn selection(&self) -> Option<&Annotation> {
        self.selected.and_then(|id| self.get(id))
    }

    pub fn unit(&self) -> f32 {
        self.picture.as_ref().map_or(1.0, Picture::unit)
    }

    fn next_badge_number(&self) -> u32 {
        self.anns.iter().filter(|a| a.kind == Kind::Badge).filter_map(|a| a.text.trim().parse::<u32>().ok()).max().unwrap_or(0) + 1
    }

    fn add(&mut self, kind: Kind, shape: Shape) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let text = match kind {
            Kind::Badge => self.next_badge_number().to_string(),
            Kind::Text => "Caption".to_string(),
            _ => String::new(),
        };
        let style: Style = kind.default_style().scaled(self.unit());
        self.anns.push(Annotation { id, kind, shape, style, text, legend: Vec::new() });
        self.selected = Some(id);
        self.dirty = true;
        id
    }

    /// Applies `f` to the selected annotation's style, as one undoable step.
    pub fn edit_style(&mut self, f: impl FnOnce(&mut Style)) {
        let Some(id) = self.selected else { return };
        self.push_history();
        if let Some(a) = self.get_mut(id) {
            f(&mut a.style);
        }
        self.dirty = true;
    }

    /// A slider moved: `start` opens one undo step for the whole drag.
    pub fn slide_style(&mut self, start: bool, f: impl FnOnce(&mut Style)) {
        let Some(id) = self.selected else { return };
        if start {
            self.push_history();
        }
        if let Some(a) = self.get_mut(id) {
            f(&mut a.style);
        }
        self.dirty = true;
    }

    pub fn delete_selected(&mut self) {
        let Some(id) = self.selected else { return };
        if self.drawing_wire == Some(id) {
            self.drawing_wire = None;
        }
        self.push_history();
        self.anns.retain(|a| a.id != id);
        self.selected = None;
        self.dirty = true;
    }

    pub fn duplicate_selected(&mut self) {
        let Some(a) = self.selection().cloned() else { return };
        self.push_history();
        let off = 20.0 * self.unit();
        let id = self.next_id;
        self.next_id += 1;
        let mut copy = Annotation { id, shape: a.translated(off, off), ..a };
        if copy.kind == Kind::Badge {
            copy.text = self.next_badge_number().to_string();
        }
        self.anns.push(copy);
        self.selected = Some(id);
        self.dirty = true;
    }

    /// Moves the selection in the stacking order: `to_end` sends it all the way.
    pub fn restack(&mut self, forward: bool, to_end: bool) {
        let Some(id) = self.selected else { return };
        let Some(i) = self.anns.iter().position(|a| a.id == id) else { return };
        let j = match (forward, to_end) {
            (true, true) => self.anns.len() - 1,
            (true, false) => (i + 1).min(self.anns.len() - 1),
            (false, true) => 0,
            (false, false) => i.saturating_sub(1),
        };
        if i == j {
            return;
        }
        self.push_history();
        let a = self.anns.remove(i);
        self.anns.insert(j, a);
        self.dirty = true;
    }

    pub fn nudge(&mut self, dx: f32, dy: f32) {
        let Some(id) = self.selected else { return };
        self.push_history();
        if let Some(a) = self.get_mut(id) {
            a.shape = a.translated(dx, dy);
        }
        self.dirty = true;
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.finish_wire();
        self.tool = tool;
        self.hover = Hover::Nothing;
    }

    /// Ends the wire being drawn, dropping its rubber-band point (and the wire if too short).
    pub fn finish_wire(&mut self) {
        let Some(id) = self.drawing_wire.take() else { return };
        let mut keep = false;
        if let Some(Annotation { shape: Shape::Polyline { points }, .. }) = self.get_mut(id) {
            points.pop();
            points.dedup();
            keep = points.len() >= 2;
        }
        if !keep {
            self.anns.retain(|a| a.id != id);
            self.drop_last_history();
            if self.selected == Some(id) {
                self.selected = None;
            }
        }
        self.dirty = true;
    }

    /// Escape: stop drawing, else drop the selection, else go back to the select tool.
    pub fn escape(&mut self) {
        if self.drawing_wire.is_some() {
            self.finish_wire();
        } else if self.selected.is_some() {
            self.selected = None;
        } else {
            self.tool = Tool::Select;
        }
    }

    // ---- Legend -----------------------------------------------------------------------------

    /// Adds a legend listing every look on the image, in the bottom-left corner.
    pub fn add_legend(&mut self) {
        let Some(pic) = &self.picture else { return };
        let (_, ih) = pic.size();
        let items = crate::legend::items_from(&self.anns, &[]);
        if items.is_empty() {
            self.status = "Draw some annotations first: the legend lists their looks.".into();
            return;
        }
        self.finish_wire();
        self.push_history();
        let id = self.next_id;
        self.next_id += 1;
        let style = Kind::Legend.default_style().scaled(self.unit());
        let margin = 30.0 * self.unit();
        let mut a =
            Annotation { id, kind: Kind::Legend, shape: Shape::Text { pos: [margin, 0.0] }, style, text: "Legend".into(), legend: items };
        let h = crate::legend::layout(&a).frame[3];
        a.shape = Shape::Text { pos: [margin, (ih - margin - h).max(0.0)] };
        self.anns.push(a);
        self.selected = Some(id);
        self.tool = Tool::Select;
        self.panel_tab = PanelTab::Text;
        self.dirty = true;
    }

    /// Re-reads the image's looks into the selected legend, keeping descriptions.
    pub fn update_legend(&mut self) {
        let Some(id) = self.selected else { return };
        let Some(old) = self.get(id).filter(|a| a.kind == Kind::Legend).map(|a| a.legend.clone()) else { return };
        let items = crate::legend::items_from(&self.anns, &old);
        self.push_history();
        if let Some(a) = self.get_mut(id) {
            a.legend = items;
        }
        self.legend_for = None;
        self.dirty = true;
    }

    pub fn remove_legend_row(&mut self, i: usize) {
        let Some(id) = self.selected else { return };
        self.push_history();
        if let Some(a) = self.get_mut(id)
            && i < a.legend.len()
        {
            a.legend.remove(i);
        }
        self.legend_for = None;
        self.dirty = true;
    }

    /// Keeps a text field per row of the selected legend, and writes typing back.
    fn sync_legend(&mut self) -> bool {
        let Some((id, labels)) = self
            .selection()
            .filter(|a| a.kind == Kind::Legend)
            .map(|a| (a.id, a.legend.iter().map(|i| i.label.clone()).collect::<Vec<_>>()))
        else {
            self.legend_for = None;
            return false;
        };
        if self.legend_for != Some(id) || self.legend_edits.len() != labels.len() {
            self.legend_edits = labels.into_iter().map(TextEdit::new).collect();
            self.legend_for = Some(id);
            self.legend_pushed = false;
            return true;
        }
        let typed: Vec<String> = self.legend_edits.iter().map(|e| e.text.clone()).collect();
        if typed == labels {
            return false;
        }
        if !self.legend_pushed {
            self.push_history();
            self.legend_pushed = true;
        }
        if let Some(a) = self.get_mut(id) {
            for (item, text) in a.legend.iter_mut().zip(typed) {
                item.label = text;
            }
        }
        self.dirty = true;
        true
    }

    // ---- Caption field ----------------------------------------------------------------------

    /// Keeps the text field on the selection's text, and writes typing back into it.
    fn sync_caption(&mut self) -> bool {
        let sel = self
            .selection()
            .filter(|a| matches!(a.kind, Kind::Text | Kind::Badge | Kind::Rect | Kind::Hatch | Kind::Legend))
            .map(|a| (a.id, a.text.clone()));
        match sel {
            Some((id, text)) if self.caption_for != Some(id) => {
                let mut edit = TextEdit::new(text);
                if std::mem::take(&mut self.focus_caption) {
                    (edit.anchor, edit.focus, edit.focused) = (0, edit.text.len(), true);
                }
                self.caption = edit;
                self.caption_for = Some(id);
                self.caption_pushed = false;
                true
            }
            Some((id, text)) if self.caption.text != text => {
                if !self.caption_pushed {
                    self.push_history();
                    self.caption_pushed = true;
                }
                let new = self.caption.text.clone();
                if let Some(a) = self.get_mut(id) {
                    a.text = new;
                }
                self.dirty = true;
                true
            }
            Some(_) => false,
            None => {
                self.caption_for = None;
                false
            }
        }
    }

    // ---- Pointer ----------------------------------------------------------------------------

    fn hit(&self, p: [f32; 2]) -> Option<u64> {
        let tol = 4.0 / self.zoom;
        self.anns.iter().rev().find(|a| a.hit(p, tol)).map(|a| a.id)
    }

    /// The selection's handle under viewport point `q`.
    fn handle_at(&self, q: [f32; 2]) -> Option<(Annotation, Handle)> {
        let a = self.selection()?;
        a.handles().iter().enumerate().find_map(|(i, h)| {
            let s = self.to_screen(*h);
            ((s[0] - q[0]).abs() <= GRAB && (s[1] - q[1]).abs() <= GRAB).then(|| (a.clone(), i))
        })
    }

    /// A drag on the canvas, in viewport points.
    pub fn drag(&mut self, d: &Drag, tx: &mut Tx) {
        let q = [d.x, d.y];
        let p = self.to_image(q);
        let shift = tx.mods().shift;
        if d.phase == Phase::Down {
            let pan = self.space || tx.button() == PointerButton::Middle;
            self.press(q, p, d.clicks, pan);
            return;
        }
        match self.gesture.clone() {
            Gesture::None => {
                if let Some(id) = self.drawing_wire {
                    self.move_wire_tip(id, p, shift);
                }
            }
            Gesture::Pan { start } => self.set_camera([start[0] + d.dx, start[1] + d.dy], self.zoom),
            Gesture::Move { orig, start, .. } => {
                let (mut dx, mut dy) = (p[0] - start[0], p[1] - start[1]);
                if shift {
                    if dx.abs() > dy.abs() { dy = 0.0 } else { dx = 0.0 }
                }
                let moved = dx != 0.0 || dy != 0.0;
                if let Some(a) = self.get_mut(orig.id) {
                    a.shape = orig.translated(dx, dy);
                }
                if let Gesture::Move { changed, .. } = &mut self.gesture {
                    *changed |= moved;
                }
                self.dirty = true;
            }
            Gesture::Handle { orig, handle, .. } => {
                if let Some(a) = self.get_mut(orig.id) {
                    a.drag_handle(&orig, handle, p, shift);
                }
                if let Gesture::Handle { changed, .. } = &mut self.gesture {
                    *changed = true;
                }
                self.dirty = true;
            }
            Gesture::Create { start } => self.drag_create(start, p, shift),
        }
        if d.phase == Phase::Up {
            self.release(d.cancelled());
        }
    }

    fn press(&mut self, q: [f32; 2], p: [f32; 2], clicks: u32, pan: bool) {
        if pan || self.picture.is_none() {
            self.gesture = Gesture::Pan { start: self.pan };
            return;
        }
        match self.tool {
            Tool::Select => {
                if let Some((orig, handle)) = self.handle_at(q) {
                    self.push_history();
                    self.gesture = Gesture::Handle { orig, handle, changed: false };
                } else if let Some(id) = self.hit(p) {
                    self.selected = Some(id);
                    let orig = self.get(id).cloned().expect("hit annotation exists");
                    self.push_history();
                    self.gesture = Gesture::Move { orig, start: p, changed: false };
                } else {
                    self.selected = None;
                    self.gesture = Gesture::Pan { start: self.pan };
                }
            }
            Tool::Draw(Kind::Wire) => {
                if let Some(id) = self.drawing_wire {
                    if clicks >= 2 {
                        self.finish_wire();
                    } else if let Some(Annotation { shape: Shape::Polyline { points }, .. }) = self.get_mut(id) {
                        let tip = *points.last().expect("a wire has points");
                        points.push(tip);
                    }
                } else {
                    self.push_history();
                    let id = self.add(Kind::Wire, Shape::Polyline { points: vec![p, p] });
                    self.drawing_wire = Some(id);
                }
                self.dirty = true;
            }
            Tool::Draw(kind) => {
                // A press on the selection's handle edits it rather than drawing anew.
                if let Some((orig, handle)) = self.handle_at(q) {
                    self.push_history();
                    self.gesture = Gesture::Handle { orig, handle, changed: false };
                    return;
                }
                self.push_history();
                let shape = match kind {
                    Kind::Arrow | Kind::Measure => Shape::Arrow { from: p, to: p },
                    Kind::Dot => Shape::Dot { center: p, radius: 12.0 * self.unit() },
                    Kind::Text => Shape::Text { pos: p },
                    _ => Shape::Rect { x: p[0], y: p[1], w: 0.0, h: 0.0 },
                };
                self.add(kind, shape);
                self.gesture = Gesture::Create { start: p };
            }
        }
    }

    fn drag_create(&mut self, start: [f32; 2], p: [f32; 2], shift: bool) {
        let Some(id) = self.selected else { return };
        let Some(a) = self.get_mut(id) else { return };
        match &mut a.shape {
            Shape::Rect { x, y, w, h } => {
                let (mut dx, mut dy) = (p[0] - start[0], p[1] - start[1]);
                if shift {
                    let m = dx.abs().max(dy.abs());
                    (dx, dy) = (m * dx.signum(), m * dy.signum());
                }
                (*x, *y, *w, *h) = (start[0].min(start[0] + dx), start[1].min(start[1] + dy), dx.abs(), dy.abs());
            }
            Shape::Arrow { from, to } => *to = if shift { snap45(*from, p) } else { p },
            Shape::Dot { center, .. } | Shape::Text { pos: center } => {
                // Dragging out a point or caption moves it while the button is down.
                *center = p;
            }
            Shape::Polyline { .. } => {}
        }
        self.dirty = true;
    }

    fn release(&mut self, cancelled: bool) {
        match std::mem::replace(&mut self.gesture, Gesture::None) {
            Gesture::Move { changed, .. } | Gesture::Handle { changed, .. } => {
                if cancelled || !changed {
                    if cancelled {
                        self.undo_in_place();
                    } else {
                        self.drop_last_history();
                    }
                }
            }
            Gesture::Create { .. } => {
                let unit = self.unit();
                let min = 4.0 / self.zoom;
                let Some(id) = self.selected else { return };
                let Some(a) = self.get_mut(id) else { return };
                match &mut a.shape {
                    // A click without a drag drops a default-sized box centred on it.
                    Shape::Rect { x, y, w, h } if *w < min && *h < min => {
                        let (dw, dh) = if a.kind == Kind::Badge { (64.0 * unit, 64.0 * unit) } else { (160.0 * unit, 110.0 * unit) };
                        (*x, *y, *w, *h) = (*x - dw / 2.0, *y - dh / 2.0, dw, dh);
                    }
                    Shape::Arrow { from, to } if (from[0] - to[0]).hypot(from[1] - to[1]) < min => {
                        self.anns.retain(|a| a.id != id);
                        self.selected = None;
                        self.drop_last_history();
                    }
                    _ => {}
                }
                if self.get(id).is_some_and(|a| a.kind == Kind::Text) {
                    // A new caption is typed right away.
                    self.caption_for = None;
                    self.focus_caption = true;
                    self.panel_tab = PanelTab::Text;
                }
                self.dirty = true;
            }
            Gesture::None | Gesture::Pan { .. } => {}
        }
    }

    /// Restores the snapshot pushed when the cancelled gesture began.
    fn undo_in_place(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.anns = prev;
            self.dirty = true;
        }
    }

    fn move_wire_tip(&mut self, id: u64, p: [f32; 2], shift: bool) {
        if let Some(Annotation { shape: Shape::Polyline { points }, .. }) = self.get_mut(id) {
            let n = points.len();
            let p = if shift && n >= 2 { snap45(points[n - 2], p) } else { p };
            if let Some(last) = points.last_mut()
                && *last != p
            {
                *last = p;
                self.dirty = true;
            }
        }
    }

    /// The pointer moved over the canvas with no primary button down. `right` says whether the
    /// right button is held: while drawing, it pans the view.
    pub fn pointer_moved(&mut self, q: [f32; 2], shift: bool, right: bool) {
        match (right && self.tool != Tool::Select && self.picture.is_some(), self.right_pan) {
            (true, None) => {
                self.right_pan = Some((q, self.pan));
            }
            (true, Some((at, start))) => {
                self.set_camera([start[0] + q[0] - at[0], start[1] + q[1] - at[1]], self.zoom);
                return;
            }
            (false, Some(_)) => self.right_pan = None,
            (false, None) => {}
        }
        if let Some(id) = self.drawing_wire {
            let p = self.to_image(q);
            self.move_wire_tip(id, p, shift);
        }
        self.hover = self.hover_at(q);
        self.pointer = Some(self.to_image(q));
    }

    pub fn pointer_left(&mut self) {
        self.pointer = None;
    }

    // ---- Clipboard --------------------------------------------------------------------------

    pub fn can_paste(&self) -> bool {
        !self.clipboard.is_empty() && self.picture.is_some()
    }

    pub fn copy(&mut self) {
        if let Some(a) = self.selection().cloned() {
            self.clipboard = vec![a];
            self.pastes = 0;
            self.status = "Copied".into();
        }
    }

    /// Pastes the clipboard centred on `at` (image pixels), else under the pointer, else
    /// stepped down and right from where it was copied.
    pub fn paste(&mut self, at: Option<[f32; 2]>) {
        if !self.can_paste() {
            return;
        }
        self.finish_wire();
        self.push_history();
        self.pastes += 1;
        let step = 20.0 * self.unit() * self.pastes as f32;
        let mut last = None;
        for a in self.clipboard.clone() {
            let [x, y, w, h] = a.bounds();
            let (dx, dy) = match at.or(self.pointer) {
                Some(p) => (p[0] - (x + w / 2.0), p[1] - (y + h / 2.0)),
                None => (step, step),
            };
            let id = self.next_id;
            self.next_id += 1;
            let mut copy = Annotation { id, shape: a.translated(dx, dy), ..a };
            if copy.kind == Kind::Badge && copy.text.trim().parse::<u32>().is_ok() {
                copy.text = self.next_badge_number().to_string();
            }
            self.anns.push(copy);
            last = Some(id);
        }
        self.selected = last;
        self.tool = Tool::Select;
        self.status = "Pasted".into();
        self.dirty = true;
    }

    // ---- Scale ------------------------------------------------------------------------------

    /// Calibrates the scale so the selected measure is as long as the typed length.
    pub fn calibrate(&mut self) {
        let Some(Annotation { shape: Shape::Arrow { from, to }, .. }) = self.selection() else { return };
        let px = (to[0] - from[0]).hypot(to[1] - from[1]);
        let typed = self.calib_field.text.trim().replace(',', ".");
        match typed.parse::<f32>().ok().and_then(|v| Scale::calibrated(px, v, self.calib_unit)) {
            Some(scale) => {
                self.scale = scale;
                self.status = format!("Scale set: {} = {}", format_args!("{} px", px.round()), scale.format(px));
            }
            None => self.status = "Type the line's real length first, such as 4.5".into(),
        }
        self.dirty = true;
    }

    pub fn clear_scale(&mut self) {
        self.scale.mm_per_px = None;
        self.dirty = true;
    }

    pub fn set_unit(&mut self, unit: Unit) {
        self.calib_unit = unit;
        self.scale.unit = unit;
        self.dirty = true;
    }

    fn hover_at(&self, q: [f32; 2]) -> Hover {
        if self.picture.is_none() || self.drawing_wire.is_some() {
            return Hover::Nothing;
        }
        if let Some((a, i)) = self.handle_at(q) {
            return match (&a.shape, i) {
                (Shape::Rect { .. }, 0 | 2) => Hover::ResizeDown,
                (Shape::Rect { .. }, 1 | 3) => Hover::ResizeUp,
                (Shape::Rect { .. }, 4 | 6) => Hover::ResizeY,
                (Shape::Rect { .. }, _) | (Shape::Dot { .. }, _) => Hover::ResizeX,
                _ => Hover::Point,
            };
        }
        if self.tool == Tool::Select && self.hit(self.to_image(q)).is_some() {
            return Hover::Body;
        }
        Hover::Nothing
    }

    /// Whether a move or resize is under way, for the cursor.
    pub fn moving(&self) -> bool {
        matches!(self.gesture, Gesture::Move { .. })
    }

    pub fn panning(&self) -> bool {
        matches!(self.gesture, Gesture::Pan { .. }) || self.right_pan.is_some()
    }

    // ---- Colours ----------------------------------------------------------------------------

    pub fn open_color_tool(&mut self, target: color::Target) {
        let Some(a) = self.selection() else { return };
        self.color_tool = Some(ColorTool::new(target, target.get(&a.style)));
    }

    /// Sets the edited colour on the selection and keeps it in the palette.
    pub fn apply_color_tool(&mut self) {
        let Some(tool) = self.color_tool.take() else { return };
        let c = tool.color();
        self.add_to_palette(c.with_alpha(255));
        self.edit_style(|st| tool.target.set(st, c));
    }

    pub fn add_to_palette(&mut self, c: crate::model::Rgba) {
        let c = c.with_alpha(255);
        if crate::model::PALETTE.contains(&c) || self.palette.contains(&c) {
            return;
        }
        self.palette.push(c);
        if self.palette.len() > 48 {
            self.palette.remove(0);
        }
        color::save_palette(&self.palette);
    }

    /// Adds `n` random flat colours, evenly spread around the hue circle.
    pub fn add_random_colors(&mut self, n: usize) {
        for c in color::random_flat(n) {
            self.add_to_palette(c);
        }
    }

    pub fn clear_palette(&mut self) {
        self.palette.clear();
        color::save_palette(&self.palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> Editor {
        let mut e = Editor::new();
        e.picture = Some(Picture { path: "t.png".into(), rgba: image::RgbaImage::new(2000, 1000) });
        e.viewport = [0.0, 0.0, 1000.0, 500.0];
        e.fit();
        e
    }

    #[test]
    fn badges_number_themselves() {
        let mut e = editor();
        e.add(Kind::Badge, Shape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        e.add(Kind::Badge, Shape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        assert_eq!(e.anns[1].text, "2");
        e.duplicate_selected();
        assert_eq!(e.anns[2].text, "3");
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut e = editor();
        e.push_history();
        e.add(Kind::Rect, Shape::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 });
        e.delete_selected();
        assert!(e.anns.is_empty());
        e.undo();
        assert_eq!(e.anns.len(), 1);
        e.undo();
        assert!(e.anns.is_empty());
        e.redo();
        assert_eq!(e.anns.len(), 1);
    }

    #[test]
    fn the_camera_maps_both_ways() {
        let e = editor();
        let p = [123.0, 456.0];
        let q = e.to_screen(p);
        let back = e.to_image(q);
        assert!((back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3);
    }
}
