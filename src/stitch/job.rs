//! The stitching job as the window sees it: the images chosen, where the result goes, a
//! background thread doing the work, and what it reports.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use image::RgbaImage;
use rune::Tx;
use rune::file_dialog::{FileDialogOptions, FileDialogResult};

use super::{Options, Progress, stitch_files};
use crate::editor::Editor;

const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "bmp"];

/// What the open menu is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One image, opened as it is.
    Single,
    /// Several overlapping photos, joined into one.
    Stitch,
}

/// Where the mosaic goes when it is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Into the editor, at most as large as the editor can show.
    Editor,
    /// To a file, as large as the photos allow.
    File,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    Idle,
    /// The images are chosen; the user decides where the mosaic goes.
    Setup,
    Running,
    Failed(String),
}

pub struct Stitcher {
    pub stage: Stage,
    pub paths: Vec<PathBuf>,
    pub output: Output,
    pub mode: Mode,
    /// Trim the mosaic to a rectangle.
    pub crop: bool,
    /// Files are being dragged over the menu's drop zone.
    pub drop_over: bool,
    progress: Option<Arc<Progress>>,
    rx: Option<Receiver<Result<super::Mosaic, String>>>,
    /// A finished mosaic waiting for the user to name its file.
    to_save: Option<RgbaImage>,
    /// A finished mosaic for the editor to open.
    ready: Option<RgbaImage>,
    /// The multiple-file picker, while it is open on its own thread.
    picked: Option<Receiver<Vec<PathBuf>>>,
    /// Wakes the idle window when a background thread has news.
    waker: Option<rune::Waker>,
    /// A line for the status bar, taken by the window.
    pub notice: Option<String>,
}

pub fn is_image(path: &Path) -> bool {
    path.extension().is_some_and(|e| IMAGE_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The images in a folder, by name.
pub fn images_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_image(p)).collect()).unwrap_or_default();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    v
}

impl Stitcher {
    pub fn new() -> Stitcher {
        Stitcher {
            stage: Stage::Idle,
            paths: Vec::new(),
            output: Output::Editor,
            mode: Mode::Single,
            crop: true,
            drop_over: false,
            progress: None,
            rx: None,
            to_save: None,
            ready: None,
            picked: None,
            waker: None,
            notice: None,
        }
    }

    pub fn set_waker(&mut self, w: rune::Waker) {
        self.waker = Some(w);
    }

    fn wake(&self) -> impl Fn() + Send + 'static {
        let w = self.waker.clone();
        move || {
            if let Some(w) = &w {
                w();
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.stage != Stage::Idle
    }

    pub fn running(&self) -> bool {
        self.stage == Stage::Running
    }

    /// Opens the menu, empty, on opening a single image.
    pub fn open(&mut self) {
        if self.running() {
            return;
        }
        self.paths.clear();
        self.mode = Mode::Single;
        self.stage = Stage::Setup;
        self.drop_over = false;
    }

    /// Opens the menu on stitching these images (they were dropped on the window).
    pub fn begin(&mut self, paths: Vec<PathBuf>) {
        if self.running() {
            return;
        }
        self.paths.clear();
        self.mode = Mode::Stitch;
        self.stage = Stage::Setup;
        self.drop_over = false;
        self.add(paths);
    }

    /// Adds images to the list, skipping the ones already there and anything that isn't an image.
    pub fn add(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            if is_image(&p) && !self.paths.contains(&p) {
                self.paths.push(p);
            }
        }
    }

    /// Opens the system's file picker to add several images at once.
    pub fn pick_images(&mut self) {
        if self.picked.is_some() {
            return;
        }
        let dialog = || rfd::FileDialog::new().set_title("Add images to stitch").add_filter("Images", &IMAGE_EXTENSIONS);
        // macOS shows its panels from the main thread only; elsewhere the picker gets its own
        // thread so the window keeps drawing.
        if cfg!(target_os = "macos") {
            if let Some(files) = dialog().pick_files() {
                self.add(files);
            }
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.picked = Some(rx);
        let wake = self.wake();
        std::thread::spawn(move || {
            let _ = tx.send(dialog().pick_files().unwrap_or_default());
            wake();
        });
    }

    pub fn picking(&self) -> bool {
        self.picked.is_some()
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.paths.len() {
            self.paths.remove(i);
        }
    }

    pub fn can_start(&self) -> bool {
        self.stage == Stage::Setup && self.mode == Mode::Stitch && self.paths.len() >= 2
    }

    pub fn close(&mut self) {
        match self.stage {
            Stage::Running => {
                if let Some(p) = &self.progress {
                    p.cancel();
                }
            }
            _ => self.stage = Stage::Idle,
        }
    }

    pub fn start(&mut self) {
        if !self.can_start() {
            return;
        }
        let paths = self.paths.clone();
        let crop = self.crop;
        let opts = match self.output {
            Output::Editor => Options { crop, ..Options::default() },
            // 200 megapixels: about 14 000 px square, and 800 MB of pixels to hold and write.
            Output::File => Options { max_side: 32_768, max_pixels: 200_000_000, crop },
        };
        let progress = Arc::new(Progress::with_waker(self.waker.clone()));
        let wake = self.wake();
        let (tx, rx) = mpsc::channel();
        self.progress = Some(progress.clone());
        self.rx = Some(rx);
        self.stage = Stage::Running;
        std::thread::spawn(move || {
            let _ = tx.send(stitch_files(&paths, &opts, &progress));
            wake();
        });
    }

    /// The stage and how far along it is, while running.
    pub fn status(&self) -> Option<(String, f32)> {
        self.progress.as_ref().filter(|_| self.running()).map(|p| (p.stage(), p.fraction()))
    }

    /// The mosaic the editor should open, once.
    pub fn take_ready(&mut self) -> Option<RgbaImage> {
        self.ready.take()
    }

    /// Called from the window's poll. True when the view needs to run again.
    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        let mut changed = self.running();
        if let Some(files) = self.picked.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.picked = None;
            self.add(files);
            changed = true;
        }
        if self.picked.is_some() {
            tx.wake_after(Duration::from_millis(150));
        }
        let finished = self.rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = finished {
            changed = true;
            self.rx = None;
            self.progress = None;
            match result {
                Ok(m) => {
                    let (w, h) = m.image.dimensions();
                    let left_out = if m.skipped.is_empty() {
                        String::new()
                    } else {
                        format!(" ({} left out: {})", m.skipped.len(), m.skipped.join(", "))
                    };
                    self.notice = Some(format!("Stitched {} images into {w}×{h} px{left_out}.", m.used));
                    self.stage = Stage::Idle;
                    match self.output {
                        Output::Editor => self.ready = Some(m.image),
                        Output::File => {
                            self.to_save = Some(m.image);
                            let options = FileDialogOptions {
                                title: "Save the mosaic".into(),
                                file_name: Some("mosaic.png".into()),
                                ..Default::default()
                            }
                            .filter("PNG", &["png"])
                            .filter("WebP (lossless)", &["webp"]);
                            tx.save_file(options, |s: &mut Editor, result, _| s.stitcher.save(result));
                        }
                    }
                }
                Err(e) if e == "cancelled" => {
                    self.stage = Stage::Idle;
                    self.notice = Some("Stitching cancelled.".into());
                }
                Err(e) => self.stage = Stage::Failed(e),
            }
        }
        if self.running() {
            tx.wake_after(Duration::from_millis(100));
        }
        changed
    }

    fn save(&mut self, result: FileDialogResult) {
        let Some(image) = self.to_save.take() else { return };
        match result {
            Ok(Some(path)) => {
                self.notice = Some(match image.save(&path) {
                    Ok(()) => format!("Saved the mosaic to {}", path.display()),
                    Err(e) => format!("Couldn't save the mosaic: {e}"),
                });
            }
            Ok(None) => self.notice = Some("The mosaic was not saved.".into()),
            Err(e) => self.notice = Some(format!("Save dialog unavailable: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_told_by_extension() {
        assert!(is_image(Path::new("a/b/Photo.JPG")));
        assert!(is_image(Path::new("tile.png")));
        assert!(!is_image(Path::new("notes.txt")));
        assert!(!is_image(Path::new("noextension")));
    }

    #[test]
    fn the_list_takes_only_new_images() {
        let mut s = Stitcher::new();
        s.open();
        assert_eq!((s.stage.clone(), s.mode), (Stage::Setup, Mode::Single));
        s.add(vec![PathBuf::from("a.png"), PathBuf::from("a.png"), PathBuf::from("notes.txt"), PathBuf::from("b.jpg")]);
        assert_eq!(s.paths, vec![PathBuf::from("a.png"), PathBuf::from("b.jpg")]);
        s.remove(0);
        s.remove(9);
        assert_eq!(s.paths, vec![PathBuf::from("b.jpg")]);
    }

    #[test]
    fn stitching_needs_two_images_and_stitch_mode() {
        let mut s = Stitcher::new();
        s.begin(vec![PathBuf::from("a.png")]);
        assert_eq!(s.mode, Mode::Stitch);
        assert!(!s.can_start());
        s.add(vec![PathBuf::from("b.png")]);
        assert!(s.can_start());
        s.mode = Mode::Single;
        assert!(!s.can_start());
        s.close();
        assert_eq!(s.stage, Stage::Idle);
    }
}
