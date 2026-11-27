//! Layr: annotate photos with boxes, hatched zones, wires, arrows, markers and captions,
//! then export the result at full resolution.
//!
//! Copyright (C) 2026 Adam Henault. Free software under the GNU GPL v3 or later; see LICENSE.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod color;
mod cursors;
mod editor;
mod font;
mod legend;
mod measure;
mod model;
mod mouse;
mod pdf;
mod render;
mod stitch;
mod ui;
mod update;
mod vector;

use rune::AppSpec;

use editor::Editor;

fn init() -> Editor {
    let mut e = Editor::new();
    // `layr photo.jpg` opens the photo; Rune's own flags (and their values) follow it.
    if let Some(path) = std::env::args().nth(1).filter(|a| !a.starts_with('-')) {
        e.load(std::path::Path::new(&path));
    }
    update::cleanup();
    e.updater.check(false);
    e
}

/// Registers the interface faces (Basalt's Space Grotesk and JetBrains Mono, and IBM Plex Sans
/// for the rail's captions) beside Rune's built-in Inter, once. `AppSpec::fonts` would replace
/// Inter, which annotations are drawn and exported in.
fn add_ui_fonts(tx: &mut rune::Tx) -> bool {
    static ADDED: std::sync::Once = std::sync::Once::new();
    let mut first = false;
    ADDED.call_once(|| {
        for face in rune_basalt::FONTS {
            tx.add_font(*face);
        }
        tx.add_font(include_bytes!("../assets/IBMPlexSans-Variable.ttf").as_slice());
        first = true;
    });
    first
}

fn poll(s: &mut Editor, _: f64, tx: &mut rune::Tx) -> bool {
    let fonts = add_ui_fonts(tx);
    let updated = s.updater.poll(tx);
    if let Some(note) = s.updater.notice.take() {
        s.status = note;
    }
    let stitching = s.stitcher.poll(tx);
    if let Some(mosaic) = s.stitcher.take_ready() {
        s.load_mosaic(mosaic);
    }
    if let Some(note) = s.stitcher.notice.take() {
        s.status = note;
    }
    s.refresh() | updated | stitching | fonts
}

fn main() {
    // The executable carries the icon on Windows (build.rs). On macOS the Dock takes it from
    // the PNG, which also covers a run outside the app bundle; Linux reads the .desktop file.
    #[cfg(windows)]
    rune::set_app_icon_resource(1);
    #[cfg(target_os = "macos")]
    if let Err(e) = rune::set_app_icon(include_bytes!("../assets/icon/icon-macos.png").as_slice()) {
        eprintln!("layr: {e}");
    }
    let spec = AppSpec {
        min_size: Some((900.0, 600.0)),
        poll: Some(poll),
        // Background work (update checks, downloads, stitching) wakes the idle window with it.
        setup: Some(|s: &mut Editor, waker| {
            s.updater.set_waker(waker.clone());
            s.stitcher.set_waker(waker);
        }),
        on_key: Some(ui::on_key),
        ..AppSpec::new("Layr", (1400.0, 900.0), init, ui::view)
    };
    // macOS: a standard titled window. Rune's default draws the content under a transparent
    // titlebar, so the traffic lights and the titlebar band would cover the top of the view.
    // Windows and Linux keep their native frame either way.
    // The interface is Basalt's dark theme: the native frame follows it.
    rune::App::new(spec)
        .chrome(rune::Chrome { full_size_content: false, ..rune::Chrome::DEFAULT })
        .color_scheme(rune::ColorScheme::Dark)
        .run();
}
