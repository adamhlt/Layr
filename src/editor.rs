//! Editor state: the loaded image, the annotations, the camera, tools, selection and history.

use std::path::{Path, PathBuf};

use rune::{Drag, Live, Phase, Pixels, PointerButton, TextEdit, Tx};

use crate::color::{self, ColorTool};

use crate::measure::{Scale, Unit};
use crate::model::{Annotation, Handle, Kind, Shape, Style, snap45};
use crate::render::{self, Area, ExportOptions};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Picks an annotation and resizes it by its handles; it doesn't move it.
    Select,
    /// Moves the annotation under the pointer, without handles.
    Move,
    Draw(Kind),
}

impl Tool {
    pub const ALL: [(Tool, &'static str, &'static str); 10] = [
        (Tool::Select, "Select", "V"),
        (Tool::Move, "Move", "W"),
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
    /// The largest rectangle without transparent pixels, `[x, y, w, h]`.
    pub filled: [u32; 4],
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
    Pan {
        start: [f32; 2],
    },
    /// Moving the selection: every selected annotation as it was when the move began.
    Move {
        origs: Vec<Annotation>,
        start: [f32; 2],
        changed: bool,
    },
    /// Drawing a selection rectangle from `start` (image pixels); `add` keeps what was selected.
    Marquee {
        start: [f32; 2],
        add: bool,
    },
    Handle {
        orig: Annotation,
        handle: Handle,
        changed: bool,
    },
    Create {
        start: [f32; 2],
    },
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
    /// Over an annotation the select tool would pick (and move if dragged).
    Pick,
}

/// Handle grab radius, in screen points.
const GRAB: f32 = 8.0;

/// The bar shown while choosing an area: its distance from the viewport's top and its height.
pub const CROP_BAR_TOP: f32 = 12.0;
pub const CROP_BAR_H: f32 = 44.0;
/// The room the bar takes at the top of the viewport, with a gap before the image.
const CROP_BAR_SPACE: f32 = CROP_BAR_TOP + CROP_BAR_H + 16.0;

/// What a press on the canvas took hold of while choosing the export area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropGrab {
    /// Drawing a new rectangle from the press.
    New,
    Move,
    /// One of [`crop_handles`].
    Handle(usize),
}

/// The handles of an `[x, y, w, h]` rectangle: corners and edge middles, clockwise from the
/// top left.
pub fn crop_handles(r: [f32; 4]) -> [[f32; 2]; 8] {
    let [x, y, w, h] = r;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    [[x, y], [cx, y], [x + w, y], [x + w, cy], [x + w, y + h], [cx, y + h], [x, y + h], [x, cy]]
}

/// One step of the history: the annotations and, for a crop, the image before it (its pixels
/// and its filled rectangle).
struct Snapshot {
    anns: Vec<Annotation>,
    image: Option<(image::RgbaImage, [u32; 4])>,
}

/// Whether two `[x, y, w, h]` rectangles touch.
fn overlaps(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] <= b[0] + b[2] && b[0] <= a[0] + a[2] && a[1] <= b[1] + b[3] && b[1] <= a[1] + a[3]
}

/// The smallest `[x, y, w, h]` rectangle holding both.
fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (x, y) = (a[0].min(b[0]), a[1].min(b[1]));
    [x, y, (a[0] + a[2]).max(b[0] + b[2]) - x, (a[1] + a[3]).max(b[1] + b[3]) - y]
}

/// The cursor's look over handle `i` of `a` (see [`Annotation::handles`] for the order).
fn handle_hover(a: &Annotation, i: Handle) -> Hover {
    match (&a.shape, i) {
        // The top-left and bottom-right corners resize along `\`, the other two along `/`.
        (Shape::Rect { .. }, 0 | 2) => Hover::ResizeDown,
        (Shape::Rect { .. }, 1 | 3) => Hover::ResizeUp,
        (Shape::Rect { .. }, 4 | 6) => Hover::ResizeY,
        (Shape::Rect { .. }, _) | (Shape::Dot { .. }, _) => Hover::ResizeX,
        _ => Hover::Point,
    }
}

/// The largest rectangle of `rgba` without transparent pixels, `[x, y, w, h]`.
fn filled_rect(rgba: &image::RgbaImage) -> [u32; 4] {
    let (w, h) = rgba.dimensions();
    if rgba.pixels().all(|p| p[3] == 255) {
        [0, 0, w, h]
    } else {
        crate::stitch::largest_filled_rect(rgba).map_or([0, 0, w, h], |(x, y, fw, fh)| [x, y, fw, fh])
    }
}

pub struct Editor {
    pub picture: Option<Picture>,
    pub anns: Vec<Annotation>,
    next_id: u64,
    pub selected: Option<u64>,
    pub tool: Tool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,

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
    /// The other selected annotations, besides `selected` (the one the panel edits).
    group: Vec<u64>,
    /// The selection rectangle being drawn with the select tool, `[x, y, w, h]` in image pixels.
    pub marquee: Option<[f32; 4]>,
    /// Whether the right-button pan went anywhere: then its release opens no context menu.
    right_moved: bool,
    /// The context menu, open at this viewport point.
    pub context_menu: Option<[f32; 2]>,

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
    /// Choosing an area on the canvas: the export's, or with `crop_image` the image's.
    pub crop_pick: bool,
    /// The area being chosen crops the image itself rather than the export.
    pub crop_image: bool,
    /// The part of the image to keep, while cropping it.
    pub image_crop: Option<[f32; 4]>,
    crop_grab: Option<(CropGrab, Option<[f32; 4]>, [f32; 2])>,
    /// The area to go back to if choosing is cancelled.
    crop_backup: Option<(Area, Option<[f32; 4]>)>,
    pub updater: crate::update::Updater,
    pub stitcher: crate::stitch::job::Stitcher,
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
            right_moved: false,
            group: Vec::new(),
            marquee: None,
            context_menu: None,
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
            crop_pick: false,
            crop_image: false,
            image_crop: None,
            crop_grab: None,
            crop_backup: None,
            updater: crate::update::Updater::new(),
            stitcher: crate::stitch::job::Stitcher::new(),
        }
    }

    // ---- Image ------------------------------------------------------------------------------

    pub fn load(&mut self, path: &Path) {
        match image::open(path) {
            Ok(img) => self.set_picture(path.to_path_buf(), img.to_rgba8()),
            Err(e) => self.status = format!("Couldn't open {}: {e}", path.display()),
        }
    }

    /// Opens a stitched mosaic. It has no file yet: `name` only gives the export its name.
    pub fn load_mosaic(&mut self, rgba: image::RgbaImage) {
        self.set_picture(PathBuf::from("mosaic.png"), rgba);
    }

    fn set_picture(&mut self, path: PathBuf, rgba: image::RgbaImage) {
        let (w, h) = rgba.dimensions();
        if w > 8192 || h > 8192 {
            self.status = format!("Image too large ({w}×{h}): 8192 px per side at most.");
            return;
        }
        self.base.set(w, h, rgba.as_raw().clone());
        self.status = format!("{} — {w}×{h} px", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let filled = filled_rect(&rgba);
        self.picture = Some(Picture { path, rgba, filled });
        self.export_open = false;
        // A drawn area belonged to the previous image.
        self.export.custom = None;
        if self.export.area == Area::Custom {
            self.export.area = Area::Whole;
        }
        (self.crop_pick, self.crop_grab, self.crop_backup) = (false, None, None);
        (self.crop_image, self.image_crop) = (false, None);
        self.anns.clear();
        self.undo.clear();
        self.redo.clear();
        self.select(None);
        self.drawing_wire = None;
        self.next_id = 1;
        self.dirty = true;
        self.fit_pending = true;
        self.fit();
    }

    /// The part of the image the export takes, `[x, y, w, h]`.
    pub fn export_rect(&self) -> Option<[u32; 4]> {
        let pic = self.picture.as_ref()?;
        let (w, h) = pic.rgba.dimensions();
        Some(match (self.export.area, self.export.custom) {
            (Area::Whole, _) | (Area::Custom, None) => [0, 0, w, h],
            (Area::Filled, _) => pic.filled,
            (Area::Custom, Some(r)) => render::clamp_rect(r, (w, h)),
        })
    }

    /// Picks the export area; a custom one without a rectangle yet is drawn right away.
    pub fn set_export_area(&mut self, area: Area) {
        if area == Area::Custom && self.export.custom.is_none() {
            self.begin_crop();
        } else {
            self.export.area = area;
        }
    }

    /// Leaves the export dialog to draw the export area on the canvas.
    pub fn begin_crop(&mut self) {
        if self.picture.is_none() {
            return;
        }
        self.finish_wire();
        self.crop_backup = Some((self.export.area, self.export.custom));
        self.export.area = Area::Custom;
        self.hover = Hover::Nothing;
        (self.crop_pick, self.export_open) = (true, false);
        self.fit();
    }

    /// Starts choosing the part of the image to keep: the whole image at first, for its edges
    /// to be dragged in.
    pub fn begin_image_crop(&mut self) {
        let Some(pic) = &self.picture else { return };
        if self.crop_pick {
            return;
        }
        let (w, h) = pic.size();
        self.finish_wire();
        self.select(None);
        self.hover = Hover::Nothing;
        (self.crop_pick, self.crop_image, self.image_crop, self.export_open) = (true, true, Some([0.0, 0.0, w, h]), false);
        self.fit();
    }

    /// What a press at viewport point `q` (image point `p`) would take hold of while choosing
    /// an area.
    fn crop_grab_at(&self, q: [f32; 2], p: [f32; 2]) -> CropGrab {
        let Some(r) = self.crop_area() else { return CropGrab::New };
        let near = |h: &[f32; 2]| {
            let s = self.to_screen(*h);
            (s[0] - q[0]).abs() <= GRAB && (s[1] - q[1]).abs() <= GRAB
        };
        if let Some(i) = crop_handles(r).iter().position(near) {
            return CropGrab::Handle(i);
        }
        // An area as large as the image has nowhere to move: a drag in it draws a new one.
        let whole = self.picture.as_ref().is_some_and(|pic| {
            let (w, h) = pic.size();
            r[0] <= 0.0 && r[1] <= 0.0 && r[0] + r[2] >= w && r[1] + r[3] >= h
        });
        let inside = (r[0]..=r[0] + r[2]).contains(&p[0]) && (r[1]..=r[1] + r[3]).contains(&p[1]);
        if inside && !whole { CropGrab::Move } else { CropGrab::New }
    }

    /// The cursor's look over an area being chosen, or while dragging it.
    fn crop_hover(&self, q: [f32; 2]) -> Hover {
        let grab = match self.crop_grab {
            Some((grab, ..)) => grab,
            None => self.crop_grab_at(q, self.to_image(q)),
        };
        match grab {
            CropGrab::New => Hover::Nothing,
            CropGrab::Move => Hover::Body,
            // `crop_handles` order: clockwise from the top left, corners and edges.
            CropGrab::Handle(0 | 4) => Hover::ResizeDown,
            CropGrab::Handle(2 | 6) => Hover::ResizeUp,
            CropGrab::Handle(1 | 5) => Hover::ResizeY,
            CropGrab::Handle(_) => Hover::ResizeX,
        }
    }

    /// The area being chosen on the canvas, in image pixels.
    pub fn crop_area(&self) -> Option<[f32; 4]> {
        if self.crop_image { self.image_crop } else { self.export.custom }
    }

    fn set_crop_area(&mut self, r: Option<[f32; 4]>) {
        if self.crop_image {
            self.image_crop = r;
        } else {
            self.export.custom = r;
        }
    }

    /// The area being chosen, in whole pixels within the image.
    pub fn crop_area_px(&self) -> Option<[u32; 4]> {
        if !self.crop_image {
            return self.export_rect();
        }
        let pic = self.picture.as_ref()?;
        Some(render::clamp_rect(self.image_crop?, pic.rgba.dimensions()))
    }

    /// Ends choosing an area. For the export, back to its dialog with the drawn area or, when
    /// `keep` is false, the one before; for the image, crops it to the area when `keep`.
    pub fn finish_crop(&mut self, keep: bool) {
        if !self.crop_pick {
            return;
        }
        self.hover = Hover::Nothing;
        if self.crop_image {
            let rect = self.crop_area_px().filter(|_| keep && self.image_crop.is_some());
            (self.crop_pick, self.crop_image, self.image_crop, self.crop_grab) = (false, false, None, None);
            self.fit();
            if let Some(r) = rect {
                self.crop_to(r);
            }
            return;
        }
        let backup = self.crop_backup.take();
        if (!keep || self.export.custom.is_none())
            && let Some((area, custom)) = backup
        {
            (self.export.area, self.export.custom) = (area, custom);
        }
        (self.crop_pick, self.crop_grab, self.export_open) = (false, None, true);
        self.fit();
    }

    /// A drag on the canvas while choosing the export area: draws, moves or resizes it.
    fn crop_drag(&mut self, d: &Drag, q: [f32; 2], p: [f32; 2], keep_ratio: bool) {
        let Some(pic) = &self.picture else { return };
        let (iw, ih) = pic.size();
        let p = [p[0].clamp(0.0, iw), p[1].clamp(0.0, ih)];
        if d.phase == Phase::Down {
            let grab = self.crop_grab_at(q, p);
            self.crop_grab = Some((grab, self.crop_area(), p));
            return;
        }
        let Some((grab, prev, start)) = self.crop_grab else { return };
        let o = prev.unwrap_or_default();
        let next = match grab {
            CropGrab::New => {
                let (x0, y0) = (start[0].min(p[0]), start[1].min(p[1]));
                [x0, y0, (p[0] - start[0]).abs(), (p[1] - start[1]).abs()]
            }
            CropGrab::Move => {
                let dx = (p[0] - start[0]).clamp(-o[0], iw - o[0] - o[2]);
                let dy = (p[1] - start[1]).clamp(-o[1], ih - o[1] - o[3]);
                [o[0] + dx, o[1] + dy, o[2], o[3]]
            }
            CropGrab::Handle(i) => {
                // `crop_handles` order: clockwise from the top left, corners and edges.
                let moves = match i {
                    0 => [true, true, false, false],
                    1 => [false, true, false, false],
                    2 => [false, true, true, false],
                    3 => [false, false, true, false],
                    4 => [false, false, true, true],
                    5 => [false, false, false, true],
                    6 => [true, false, false, true],
                    _ => [true, false, false, false],
                };
                let r = crate::model::resize_rect(o, moves, p, keep_ratio);
                // Kept proportions may reach past the image: the area stops at its edges.
                let (x0, y0) = (r[0].max(0.0), r[1].max(0.0));
                let (x1, y1) = ((r[0] + r[2]).min(iw), (r[1] + r[3]).min(ih));
                [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
            }
        };
        self.set_crop_area(Some(next));
        if d.phase == Phase::Up {
            // A click, or a cancelled drag, leaves the area as it was.
            let tiny = next[2] < 2.0 || next[3] < 2.0;
            if d.cancelled() || tiny {
                self.set_crop_area(prev);
            }
            self.crop_grab = None;
        }
    }

    /// Crops the image to `[x, y, w, h]` (whole pixels inside it), moving the annotations with
    /// it, as one undoable step.
    fn crop_to(&mut self, [x, y, w, h]: [u32; 4]) {
        let Some(pic) = &self.picture else { return };
        if [x, y, w, h] == [0, 0, pic.rgba.width(), pic.rgba.height()] {
            return;
        }
        let cropped = image::imageops::crop_imm(&pic.rgba, x, y, w, h).to_image();
        let filled = filled_rect(&cropped);
        let before = self.replace_image(cropped, filled);
        self.undo.push(Snapshot { anns: self.anns.clone(), image: Some(before) });
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
        let (dx, dy) = (-(x as f32), -(y as f32));
        for a in &mut self.anns {
            a.shape = a.translated(dx, dy);
        }
        self.status = format!("Cropped to {w}×{h} px");
    }

    /// Swaps in new pixels for the image, returning the old ones. A drawn export area belonged
    /// to the old pixels, so it goes.
    fn replace_image(&mut self, rgba: image::RgbaImage, filled: [u32; 4]) -> (image::RgbaImage, [u32; 4]) {
        let pic = self.picture.as_mut().expect("an image to replace");
        self.base.set(rgba.width(), rgba.height(), rgba.as_raw().clone());
        let old = (std::mem::replace(&mut pic.rgba, rgba), std::mem::replace(&mut pic.filled, filled));
        self.export.custom = None;
        if self.export.area == Area::Custom {
            self.export.area = Area::Whole;
        }
        self.dirty = true;
        self.fit_pending = true;
        self.fit();
        old
    }

    pub fn export(&mut self, path: &Path) {
        self.finish_wire();
        let Some(rect) = self.export_rect() else { return };
        let Some(pic) = &self.picture else { return };
        self.status = match render::export(&pic.rgba, &self.anns, &self.scale, &self.export, rect, path) {
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

    /// Fits the whole image in the viewport; below the bar while choosing an area.
    pub fn fit(&mut self) {
        let Some(pic) = &self.picture else { return };
        let [_, _, vw, vh] = self.viewport;
        let top = if self.crop_pick { CROP_BAR_SPACE } else { 0.0 };
        let h = vh - top;
        if vw < 1.0 || h < 1.0 {
            return;
        }
        self.fit_pending = false;
        let (iw, ih) = pic.size();
        let zoom = (vw / iw).min(h / ih) * 0.96;
        self.set_camera([(vw - iw * zoom) / 2.0, top + (h - ih * zoom) / 2.0], zoom);
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
        self.undo.push(Snapshot { anns: self.anns.clone(), image: None });
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
            let now = self.restore(prev);
            self.redo.push(now);
            self.after_history();
        }
    }

    pub fn redo(&mut self) {
        self.finish_wire();
        if let Some(next) = self.redo.pop() {
            let now = self.restore(next);
            self.undo.push(now);
            self.after_history();
        }
    }

    /// Puts `snap` in place, returning what it replaced (the image too, for a crop).
    fn restore(&mut self, snap: Snapshot) -> Snapshot {
        if self.crop_image {
            self.finish_crop(false);
        }
        let image = snap.image.map(|(rgba, filled)| self.replace_image(rgba, filled));
        Snapshot { anns: std::mem::replace(&mut self.anns, snap.anns), image }
    }

    fn after_history(&mut self) {
        let anns = &self.anns;
        self.group.retain(|id| anns.iter().any(|a| a.id == *id));
        if self.selected.is_some_and(|id| self.get(id).is_none()) {
            self.select(None);
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

    /// Selects `id` alone (or nothing).
    pub fn select(&mut self, id: Option<u64>) {
        self.selected = id;
        self.group.clear();
    }

    /// Every selected annotation's id, the panel's one first.
    pub fn picked(&self) -> Vec<u64> {
        self.selected.into_iter().chain(self.group.iter().copied()).filter(|id| self.get(*id).is_some()).collect()
    }

    /// Selects `ids`, the first one in the panel.
    fn select_all(&mut self, ids: Vec<u64>) {
        let mut ids = ids.into_iter();
        self.selected = ids.next();
        self.group = ids.collect();
    }

    /// Adds `id` to the selection, or takes it out if it is in it.
    fn toggle_picked(&mut self, id: u64) {
        let mut ids = self.picked();
        if let Some(i) = ids.iter().position(|x| *x == id) {
            ids.remove(i);
        } else {
            ids.push(id);
        }
        self.select_all(ids);
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
        self.select(Some(id));
        self.dirty = true;
        id
    }

    /// Applies `f` to every selected annotation's style, as one undoable step.
    pub fn edit_style(&mut self, f: impl Fn(&mut Style)) {
        let ids = self.picked();
        if ids.is_empty() {
            return;
        }
        self.push_history();
        for a in self.anns.iter_mut().filter(|a| ids.contains(&a.id)) {
            f(&mut a.style);
        }
        self.dirty = true;
    }

    /// A slider moved: `start` opens one undo step for the whole drag.
    pub fn slide_style(&mut self, start: bool, f: impl Fn(&mut Style)) {
        let ids = self.picked();
        if ids.is_empty() {
            return;
        }
        if start {
            self.push_history();
        }
        for a in self.anns.iter_mut().filter(|a| ids.contains(&a.id)) {
            f(&mut a.style);
        }
        self.dirty = true;
    }

    pub fn delete_selected(&mut self) {
        let ids = self.picked();
        if ids.is_empty() {
            return;
        }
        if self.drawing_wire.is_some_and(|w| ids.contains(&w)) {
            self.drawing_wire = None;
        }
        self.push_history();
        self.anns.retain(|a| !ids.contains(&a.id));
        self.select(None);
        self.dirty = true;
    }

    pub fn duplicate_selected(&mut self) {
        let ids = self.picked();
        if ids.is_empty() {
            return;
        }
        self.push_history();
        let off = 20.0 * self.unit();
        let mut copies = Vec::new();
        for src in ids {
            let Some(a) = self.get(src).cloned() else { continue };
            let id = self.next_id;
            self.next_id += 1;
            let mut copy = Annotation { id, shape: a.translated(off, off), ..a };
            if copy.kind == Kind::Badge {
                copy.text = self.next_badge_number().to_string();
            }
            self.anns.push(copy);
            copies.push(id);
        }
        self.select_all(copies);
        self.dirty = true;
    }

    /// Moves the selection in the stacking order, keeping its own order: `to_end` sends it all
    /// the way.
    pub fn restack(&mut self, forward: bool, to_end: bool) {
        let ids = self.picked();
        if ids.is_empty() {
            return;
        }
        let mut order: Vec<u64> = self.anns.iter().map(|a| a.id).collect();
        let before = order.clone();
        if to_end {
            let (picked, rest): (Vec<u64>, Vec<u64>) = order.into_iter().partition(|id| ids.contains(id));
            order = if forward { rest.into_iter().chain(picked).collect() } else { picked.into_iter().chain(rest).collect() };
        } else if forward {
            // One step up: from the top down, each selected one swaps with an unselected one above.
            for i in (0..order.len().saturating_sub(1)).rev() {
                if ids.contains(&order[i]) && !ids.contains(&order[i + 1]) {
                    order.swap(i, i + 1);
                }
            }
        } else {
            for i in 1..order.len() {
                if ids.contains(&order[i]) && !ids.contains(&order[i - 1]) {
                    order.swap(i, i - 1);
                }
            }
        }
        if order == before {
            return;
        }
        self.push_history();
        let mut anns = std::mem::take(&mut self.anns);
        self.anns = order.iter().filter_map(|id| anns.iter().position(|a| a.id == *id).map(|i| anns.swap_remove(i))).collect();
        self.dirty = true;
    }

    pub fn nudge(&mut self, dx: f32, dy: f32) {
        let ids = self.picked();
        if ids.is_empty() {
            return;
        }
        self.push_history();
        for a in self.anns.iter_mut().filter(|a| ids.contains(&a.id)) {
            a.shape = a.translated(dx, dy);
        }
        self.dirty = true;
    }

    pub fn set_tool(&mut self, tool: Tool) {
        if self.crop_image {
            self.finish_crop(false);
        }
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
                self.select(None);
            }
        }
        self.dirty = true;
    }

    /// Escape: stop drawing, else drop the selection, else go back to the select tool.
    pub fn escape(&mut self) {
        if self.drawing_wire.is_some() {
            self.finish_wire();
        } else if self.selected.is_some() {
            self.select(None);
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
        self.select(Some(id));
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
        // The move tool shows no handles.
        if self.tool == Tool::Move {
            return None;
        }
        // Several selected annotations show outlines, not handles.
        if !self.group.is_empty() {
            return None;
        }
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
        // Ctrl (or ⌘) resizes without changing the proportions.
        let keep_ratio = tx.mods().ctrl || tx.mods().cmd;
        // Choosing the export area takes the canvas, except to pan it.
        if self.crop_pick && self.picture.is_some() {
            let pan = d.phase == Phase::Down && (self.space || tx.button() == PointerButton::Middle);
            if !pan && !matches!(self.gesture, Gesture::Pan { .. }) {
                self.crop_drag(d, q, p, keep_ratio);
                self.hover = self.hover_at(q);
                return;
            }
        }
        if d.phase == Phase::Down {
            let pan = self.space || tx.button() == PointerButton::Middle;
            self.press(q, p, d.clicks, pan, shift);
            self.hover = self.hover_at(q);
            return;
        }
        match self.gesture.clone() {
            Gesture::None => {
                if let Some(id) = self.drawing_wire {
                    self.move_wire_tip(id, p, shift);
                }
            }
            Gesture::Pan { start } => self.set_camera([start[0] + d.dx, start[1] + d.dy], self.zoom),
            Gesture::Move { origs, start, .. } => {
                let (mut dx, mut dy) = (p[0] - start[0], p[1] - start[1]);
                if shift {
                    if dx.abs() > dy.abs() { dy = 0.0 } else { dx = 0.0 }
                }
                let moved = dx != 0.0 || dy != 0.0;
                for orig in &origs {
                    if let Some(a) = self.get_mut(orig.id) {
                        a.shape = orig.translated(dx, dy);
                    }
                }
                if let Gesture::Move { changed, .. } = &mut self.gesture {
                    *changed |= moved;
                }
                self.dirty = true;
            }
            Gesture::Handle { orig, handle, .. } => {
                if let Some(a) = self.get_mut(orig.id) {
                    a.drag_handle(&orig, handle, p, shift, keep_ratio);
                }
                if let Gesture::Handle { changed, .. } = &mut self.gesture {
                    *changed = true;
                }
                self.dirty = true;
            }
            Gesture::Create { start } => self.drag_create(start, p, shift),
            Gesture::Marquee { start, .. } => {
                self.marquee = Some([start[0].min(p[0]), start[1].min(p[1]), (p[0] - start[0]).abs(), (p[1] - start[1]).abs()]);
                self.dirty = true;
            }
        }
        if d.phase == Phase::Up {
            self.release(d.cancelled());
        }
        // The pointer may not report moves while a button is down: the look follows the drag.
        self.hover = self.hover_at(q);
    }

    /// Takes hold of annotation `id` at `p` to move it. A selected one moves the whole
    /// selection (and comes first, into the panel); another one is selected alone.
    fn grab(&mut self, id: u64, p: [f32; 2]) {
        let mut ids = self.picked();
        match ids.iter().position(|x| *x == id) {
            Some(i) => {
                ids.remove(i);
                ids.insert(0, id);
                self.select_all(ids);
            }
            None => self.select(Some(id)),
        }
        let origs = self.picked().iter().filter_map(|id| self.get(*id).cloned()).collect();
        self.push_history();
        self.gesture = Gesture::Move { origs, start: p, changed: false };
    }

    /// A press on the canvas. `add` (Shift) adds to the selection rather than replacing it.
    fn press(&mut self, q: [f32; 2], p: [f32; 2], clicks: u32, pan: bool, add: bool) {
        if pan || self.picture.is_none() {
            self.gesture = Gesture::Pan { start: self.pan };
            return;
        }
        match self.tool {
            Tool::Select => {
                if let Some((orig, handle)) = self.handle_at(q).filter(|_| !add) {
                    self.push_history();
                    self.gesture = Gesture::Handle { orig, handle, changed: false };
                } else if let Some(id) = self.hit(p) {
                    // Shift adds or takes out; otherwise the press selects and a drag moves.
                    if add { self.toggle_picked(id) } else { self.grab(id, p) }
                } else {
                    // Elsewhere a drag draws a selection rectangle.
                    if !add {
                        self.select(None);
                    }
                    self.marquee = None;
                    self.gesture = Gesture::Marquee { start: p, add };
                }
            }
            Tool::Move => match self.hit(p) {
                Some(id) => self.grab(id, p),
                None => self.gesture = Gesture::Pan { start: self.pan },
            },
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
                        self.select(None);
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
            Gesture::Marquee { add, .. } => {
                if let Some(r) = self.marquee.take() {
                    // What the rectangle touches, in stacking order.
                    let hits: Vec<u64> = self.anns.iter().filter(|a| overlaps(a.bounds(), r)).map(|a| a.id).collect();
                    let mut ids = if add { self.picked() } else { Vec::new() };
                    ids.extend(hits.into_iter().filter(|id| !ids.contains(id)).collect::<Vec<_>>());
                    self.select_all(ids);
                }
                self.dirty = true;
            }
            Gesture::None | Gesture::Pan { .. } => {}
        }
    }

    /// Restores the snapshot pushed when the cancelled gesture began.
    fn undo_in_place(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.anns = prev.anns;
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
                self.right_moved = false;
            }
            (true, Some((at, start))) => {
                self.right_moved |= (q[0] - at[0]).hypot(q[1] - at[1]) > 3.0;
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
        let ids = self.picked();
        if !ids.is_empty() {
            self.clipboard = self.anns.iter().filter(|a| ids.contains(&a.id)).cloned().collect();
            self.pastes = 0;
            self.status = "Copied".into();
        }
    }

    /// Copies the selection, then deletes it.
    pub fn cut(&mut self) {
        if self.selection().is_some() {
            self.copy();
            self.delete_selected();
            self.status = "Cut".into();
        }
    }

    /// A right-click on the canvas at viewport point `q`: selects what is there and opens the
    /// context menu, unless the right button panned the view.
    pub fn open_context_menu(&mut self, q: [f32; 2]) {
        if std::mem::take(&mut self.right_moved) || self.picture.is_none() || self.crop_pick {
            return;
        }
        self.finish_wire();
        if let Some(id) = self.hit(self.to_image(q))
            && !self.picked().contains(&id)
        {
            self.select(Some(id));
        }
        self.context_menu = Some(q);
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
        // Several annotations move together, keeping their layout: their bounds are centred.
        let [x, y, w, h] = self.clipboard.iter().map(Annotation::bounds).reduce(union).unwrap_or_default();
        let (dx, dy) = match at.or(self.pointer) {
            Some(p) => (p[0] - (x + w / 2.0), p[1] - (y + h / 2.0)),
            None => (step, step),
        };
        let mut pasted = Vec::new();
        for a in self.clipboard.clone() {
            let id = self.next_id;
            self.next_id += 1;
            let mut copy = Annotation { id, shape: a.translated(dx, dy), ..a };
            if copy.kind == Kind::Badge && copy.text.trim().parse::<u32>().is_ok() {
                copy.text = self.next_badge_number().to_string();
            }
            self.anns.push(copy);
            pasted.push(id);
        }
        self.select_all(pasted);
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
        if self.crop_pick {
            return self.crop_hover(q);
        }
        // A handle being dragged keeps its look, wherever the pointer is.
        if let Gesture::Handle { orig, handle, .. } = &self.gesture {
            return handle_hover(orig, *handle);
        }
        if let Some((a, i)) = self.handle_at(q) {
            return handle_hover(&a, i);
        }
        if self.hit(self.to_image(q)).is_some() {
            match self.tool {
                Tool::Select => return Hover::Pick,
                Tool::Move => return Hover::Body,
                Tool::Draw(_) => {}
            }
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
        e.picture = Some(Picture { path: "t.png".into(), rgba: image::RgbaImage::new(2000, 1000), filled: [0, 0, 2000, 1000] });
        e.viewport = [0.0, 0.0, 1000.0, 500.0];
        e.fit();
        e
    }

    #[test]
    fn the_export_area_follows_the_choice() {
        let mut e = editor();
        e.picture.as_mut().unwrap().filled = [10, 20, 1500, 900];
        assert_eq!(e.export_rect(), Some([0, 0, 2000, 1000]));
        e.set_export_area(Area::Filled);
        assert_eq!(e.export_rect(), Some([10, 20, 1500, 900]));
        e.export.custom = Some([100.4, -50.0, 300.0, 200.0]);
        e.set_export_area(Area::Custom);
        assert!(!e.crop_pick, "an area already drawn is reused");
        assert_eq!(e.export_rect(), Some([100, 0, 300, 150]), "cut to the image");
    }

    #[test]
    fn a_cancelled_area_restores_the_previous_one() {
        let mut e = editor();
        e.export_open = true;
        e.set_export_area(Area::Custom);
        assert!(e.crop_pick && !e.export_open, "nothing drawn yet: the canvas opens");
        e.export.custom = Some([0.0, 0.0, 50.0, 50.0]);
        e.finish_crop(false);
        assert_eq!((e.export.area, e.export.custom), (Area::Whole, None));
        assert!(e.export_open && !e.crop_pick);
        e.begin_crop();
        e.export.custom = Some([0.0, 0.0, 50.0, 50.0]);
        e.finish_crop(true);
        assert_eq!((e.export.area, e.export_rect()), (Area::Custom, Some([0, 0, 50, 50])));
    }

    #[test]
    fn a_new_image_forgets_the_drawn_area() {
        let mut e = editor();
        e.export.custom = Some([0.0, 0.0, 50.0, 50.0]);
        e.export.area = Area::Custom;
        e.load_mosaic(image::RgbaImage::from_pixel(40, 30, image::Rgba([1, 2, 3, 255])));
        assert_eq!((e.export.area, e.export.custom, e.export_rect()), (Area::Whole, None, Some([0, 0, 40, 30])));
    }

    #[test]
    fn cropping_the_image_moves_the_annotations_and_undoes() {
        let mut e = editor();
        e.add(Kind::Rect, Shape::Rect { x: 500.0, y: 300.0, w: 10.0, h: 10.0 });
        e.begin_image_crop();
        assert!(e.crop_pick && e.crop_image);
        e.image_crop = Some([400.4, 200.0, 800.0, 2000.0]);
        e.finish_crop(true);
        assert!(!e.crop_pick && !e.crop_image);
        assert_eq!(e.picture.as_ref().unwrap().rgba.dimensions(), (800, 800), "cut to the image");
        assert_eq!(e.anns[0].shape, Shape::Rect { x: 100.0, y: 100.0, w: 10.0, h: 10.0 });
        e.undo();
        assert_eq!(e.picture.as_ref().unwrap().rgba.dimensions(), (2000, 1000));
        assert_eq!(e.anns[0].shape, Shape::Rect { x: 500.0, y: 300.0, w: 10.0, h: 10.0 });
        e.redo();
        assert_eq!(e.picture.as_ref().unwrap().rgba.dimensions(), (800, 800));
        assert_eq!(e.anns[0].shape, Shape::Rect { x: 100.0, y: 100.0, w: 10.0, h: 10.0 });
    }

    #[test]
    fn the_image_crop_starts_on_the_whole_image_and_shows_its_handles() {
        let mut e = editor();
        e.begin_image_crop();
        assert_eq!(e.image_crop, Some([0.0, 0.0, 2000.0, 1000.0]));
        let at = |e: &Editor, p: [f32; 2]| e.hover_at(e.to_screen(p));
        assert_eq!(at(&e, [0.0, 0.0]), Hover::ResizeDown);
        assert_eq!(at(&e, [2000.0, 0.0]), Hover::ResizeUp);
        assert_eq!(at(&e, [1000.0, 1000.0]), Hover::ResizeY);
        assert_eq!(at(&e, [0.0, 500.0]), Hover::ResizeX);
        assert_eq!(at(&e, [1000.0, 500.0]), Hover::Nothing, "the whole image: a drag draws anew");
        e.image_crop = Some([500.0, 200.0, 800.0, 600.0]);
        assert_eq!(at(&e, [900.0, 500.0]), Hover::Body, "inside a smaller area: a drag moves it");
        assert_eq!(at(&e, [100.0, 100.0]), Hover::Nothing);
        e.finish_crop(true);
        assert_eq!(e.picture.as_ref().unwrap().rgba.dimensions(), (800, 600));
    }

    #[test]
    fn choosing_an_area_keeps_the_image_clear_of_the_bar() {
        let mut e = editor();
        e.begin_image_crop();
        let top = e.to_screen([0.0, 0.0])[1];
        assert!(top >= CROP_BAR_TOP + CROP_BAR_H + 16.0, "image top at {top}");
        e.finish_crop(false);
        assert!(e.to_screen([0.0, 0.0])[1] < CROP_BAR_TOP + CROP_BAR_H, "back to the full view");
    }

    #[test]
    fn handles_show_the_way_they_resize() {
        let mut e = editor();
        e.add(Kind::Rect, Shape::Rect { x: 100.0, y: 100.0, w: 400.0, h: 200.0 });
        let at = |e: &Editor, p: [f32; 2]| e.hover_at(e.to_screen(p));
        assert_eq!(at(&e, [100.0, 100.0]), Hover::ResizeDown);
        assert_eq!(at(&e, [500.0, 300.0]), Hover::ResizeDown);
        assert_eq!(at(&e, [500.0, 100.0]), Hover::ResizeUp);
        assert_eq!(at(&e, [100.0, 300.0]), Hover::ResizeUp);
        assert_eq!(at(&e, [300.0, 100.0]), Hover::ResizeY);
        assert_eq!(at(&e, [500.0, 200.0]), Hover::ResizeX);
        assert_eq!(at(&e, [300.0, 200.0]), Hover::Pick, "the select tool picks");
        e.set_tool(Tool::Move);
        assert_eq!(at(&e, [300.0, 200.0]), Hover::Body, "the move tool does");
        assert_eq!(at(&e, [100.0, 100.0]), Hover::Body, "and shows no handles");
    }

    #[test]
    fn select_and_move_both_move() {
        let mut e = editor();
        let id = e.add(Kind::Rect, Shape::Rect { x: 100.0, y: 100.0, w: 400.0, h: 200.0 });
        e.selected = None;
        let (p, q) = ([300.0, 200.0], e.to_screen([300.0, 200.0]));
        e.press(q, p, 1, false, false);
        assert_eq!(e.selected, Some(id), "a press selects");
        assert!(e.moving(), "and a drag would move it");
        e.release(false);
        e.selected = None;
        e.set_tool(Tool::Move);
        e.press(q, p, 1, false, false);
        assert_eq!(e.selected, Some(id));
        assert!(e.moving(), "the move tool takes hold of it");
        e.release(false);
        assert!(e.handle_at(e.to_screen([100.0, 100.0])).is_none(), "no handles to resize with");
    }

    /// Three boxes side by side, nothing selected.
    fn three_boxes() -> (Editor, [u64; 3]) {
        let mut e = editor();
        let ids = [100.0, 400.0, 700.0].map(|x| e.add(Kind::Rect, Shape::Rect { x, y: 100.0, w: 100.0, h: 100.0 }));
        e.select(None);
        (e, ids)
    }

    /// A select-tool drag from `a` to `b` (image pixels).
    fn marquee(e: &mut Editor, a: [f32; 2], b: [f32; 2], add: bool) {
        e.press(e.to_screen(a), a, 1, false, add);
        if let Gesture::Marquee { start, .. } = e.gesture {
            e.marquee = Some([start[0].min(b[0]), start[1].min(b[1]), (b[0] - start[0]).abs(), (b[1] - start[1]).abs()]);
        }
        e.release(false);
    }

    #[test]
    fn a_selection_rectangle_picks_what_it_touches() {
        let (mut e, [a, b, c]) = three_boxes();
        marquee(&mut e, [50.0, 50.0], [450.0, 150.0], false);
        assert_eq!(e.picked(), vec![a, b], "the first two, the third untouched");
        assert!(e.marquee.is_none(), "the rectangle goes away");
        marquee(&mut e, [650.0, 50.0], [750.0, 150.0], true);
        assert_eq!(e.picked(), vec![a, b, c], "Shift adds to the selection");
        marquee(&mut e, [0.0, 600.0], [10.0, 610.0], false);
        assert!(e.picked().is_empty(), "an empty rectangle selects nothing");
    }

    #[test]
    fn shift_click_adds_and_takes_out() {
        let (mut e, [a, b, _]) = three_boxes();
        for (x, add) in [(150.0, false), (450.0, true)] {
            e.press(e.to_screen([x, 150.0]), [x, 150.0], 1, false, add);
            e.release(false);
        }
        assert_eq!(e.picked(), vec![a, b]);
        e.press(e.to_screen([150.0, 150.0]), [150.0, 150.0], 1, false, true);
        assert_eq!(e.picked(), vec![b]);
    }

    #[test]
    fn several_selected_move_delete_and_copy_together() {
        let (mut e, [a, b, c]) = three_boxes();
        marquee(&mut e, [50.0, 50.0], [450.0, 150.0], false);
        assert!(e.handle_at(e.to_screen([100.0, 100.0])).is_none(), "a group shows no handles");
        e.nudge(10.0, 0.0);
        assert_eq!(e.get(a).unwrap().bounds()[0], 110.0);
        assert_eq!(e.get(b).unwrap().bounds()[0], 410.0);
        assert_eq!(e.get(c).unwrap().bounds()[0], 700.0, "the unselected one stays");
        e.edit_style(|st| st.stroke_width = 9.0);
        assert!(e.get(a).unwrap().style.stroke_width == 9.0 && e.get(b).unwrap().style.stroke_width == 9.0);
        e.copy();
        e.paste(Some([1000.0, 800.0]));
        let pasted: Vec<_> = e.picked().iter().map(|id| e.get(*id).unwrap().bounds()).collect();
        assert_eq!(pasted.len(), 2, "both pasted, both selected");
        assert_eq!(pasted[1][0] - pasted[0][0], 300.0, "keeping their layout");
        e.delete_selected();
        assert_eq!(e.anns.len(), 3);
    }

    #[test]
    fn several_selected_restack_in_their_order() {
        let (mut e, [a, b, c]) = three_boxes();
        e.select_all(vec![a, b]);
        e.restack(true, true);
        assert_eq!(e.anns.iter().map(|x| x.id).collect::<Vec<_>>(), vec![c, a, b]);
        e.restack(false, false);
        assert_eq!(e.anns.iter().map(|x| x.id).collect::<Vec<_>>(), vec![a, b, c]);
    }

    #[test]
    fn cut_copies_then_deletes() {
        let mut e = editor();
        e.add(Kind::Rect, Shape::Rect { x: 100.0, y: 100.0, w: 40.0, h: 20.0 });
        e.cut();
        assert!(e.anns.is_empty() && e.selected.is_none());
        assert!(e.can_paste());
        e.paste(Some([500.0, 500.0]));
        assert_eq!(e.anns.len(), 1);
        assert_eq!(e.anns[0].bounds(), [480.0, 490.0, 40.0, 20.0], "pasted centred on the point");
    }

    #[test]
    fn a_right_click_opens_the_menu_on_what_it_hits() {
        let mut e = editor();
        let id = e.add(Kind::Rect, Shape::Rect { x: 100.0, y: 100.0, w: 400.0, h: 200.0 });
        e.selected = None;
        let q = e.to_screen([300.0, 200.0]);
        e.open_context_menu(q);
        assert_eq!(e.context_menu, Some(q));
        assert_eq!(e.selected, Some(id), "what was right-clicked is selected");
        // A right-button pan that went somewhere opens nothing when the button comes up.
        e.context_menu = None;
        e.set_tool(Tool::Draw(Kind::Rect));
        e.pointer_moved([10.0, 10.0], false, true);
        e.pointer_moved([60.0, 40.0], false, true);
        e.open_context_menu([60.0, 40.0]);
        assert_eq!(e.context_menu, None);
    }

    #[test]
    fn a_cancelled_image_crop_changes_nothing() {
        let mut e = editor();
        e.begin_image_crop();
        e.image_crop = Some([0.0, 0.0, 50.0, 50.0]);
        e.finish_crop(false);
        assert_eq!(e.picture.as_ref().unwrap().rgba.dimensions(), (2000, 1000));
        assert!(!e.can_undo());
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
