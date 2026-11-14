# Layr

An image annotation editor (circuit board photos, screenshots, diagrams) written in Rust with the
[Rune](https://github.com/anomalyco/rune) UI library.

Open an image, draw rectangles, hatched areas, numbered badges, wires, arrows, dots and captions
over it, then export the annotated image as PNG or JPEG at its original resolution.

## Features

| Tool | Key | Use |
| --- | --- | --- |
| Select | `V` | Click to select, drag to move, drag the handles to resize |
| Rectangle | `R` | Drag (Shift: square), or click for a default size |
| Hatch | `H` | A hatched area (angle, spacing, line width and opacity) |
| Badge | `N` | A numbered badge that counts up by itself: circle, rounded, square, diamond or hexagon |
| Wire | `L` | Click each point, double-click or Enter to finish (Shift: 45° steps) |
| Arrow | `A` | Drag from the tail to the tip (Shift: 45° steps) |
| Dot | `P` | Click to drop a dot (drag its handle to change its size) |
| Text | `T` | A multi-line caption with a translucent background and a coloured frame |
| Measure | `M` | A dimension line labelled with its real length |
| Legend | `G` | Adds a legend: one row per look used on the image, each with a description you edit |

The right-hand panel shows while an annotation is selected. Its Style, Text and Arrange tabs set:
- outline colour, width and opacity;
- fill colour and opacity;
- corner radius and hatching;
- font (any installed family), bold, size and text colour;
- stacking order; you can also duplicate and delete it.

The **+** at the end of each colour row opens the colour editor:
- **Input**: hue, saturation, lightness and opacity sliders, or a field that takes `#RGB`,
  `#RRGGBB(AA)`, `rgb()`, `hsl()` or any HTML colour name.
- **Random flat colours**: pick one, or add a batch to the palette.
- **Your palette**: the colours you add are saved between runs.

The canvas draws annotations as vectors, so they stay sharp at any zoom. Export draws them with
the same geometry and fonts.

**Real sizes.** Draw a measure across something whose length you know, type that length, pick
the unit (µm, mm, cm, m, in) and press **Set scale**. Every measure then shows its real length,
and a box with **Show real size** shows its width and height.

- Undo / redo: `Ctrl+Z` / `Ctrl+Y`.
- **Copy and paste**: `Ctrl+C`, `Ctrl+V` (pastes under the pointer), and `Ctrl+D` to
  duplicate.
- **Legend**: **Update from annotations** adds rows for new looks and keeps the descriptions
  you wrote.
- **Zoom**: the mouse wheel zooms around the pointer; `Ctrl+F` fits the image to the window.
- **Pan**: Space + drag, the middle button, the right button while a drawing tool is active, or a
  drag on an empty spot with the select tool.
- The cursor shows what a drag will do: move, or resize along an edge or a corner.
- Open with `Ctrl+O`, by dropping an image on the window, or with `layr photo.jpg`.
- **Export** with `Ctrl+E`:
  - **Formats**: PNG, JPEG (with a quality setting), lossless WebP, or PDF.
  - **Options**: scaled from 25 % to 200 %, with or without the photo (a transparent overlay).
  - **PDF**: annotations stay vectors and their text is real, embedded-font text you can
    select, search and copy.
- Default stroke widths and text sizes follow the image's resolution.

Reads PNG, JPEG, WebP and BMP, up to 8192 px per side.

## Building

`rust-toolchain.toml` pins the Rust version; rustup installs it on first use.

```bash
cargo run --release
```

- **Windows**: the Visual Studio C++ build tools (MSVC toolchain).
- **Linux**: `sudo apt-get install libxkbcommon-dev libwayland-dev` to build; running also needs
  `libxkbcommon-x11-0` and a Vulkan driver. Right-button panning reads the button through X11, so
  it works under X11 and XWayland but not in a pure Wayland session.
- **macOS**: the Xcode Command Line Tools.

Rune is pre-release, so `Cargo.toml` pins one revision. To update it, change the same `rev` on
`rune` and on every crate under `[patch.crates-io]`.

Debug builds can render the app without a window, for screenshots or scripted runs:

```bash
cargo run -- photo.jpg --snap capture.png --size 1400x900
```

## Releases

`.github/workflows/release.yml` builds Windows (x64), Linux (x64) and macOS (a universal Intel +
Apple silicon app), then publishes the three archives in one GitHub release:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The macOS app is ad-hoc signed but not notarized. The first time, right-click → Open, or run
`xattr -dr com.apple.quarantine "Layr.app"`.

## Icon

`assets/logo/1-overlap.svg` is the logo. `python packaging/make_icons.py` (Pillow) renders it on
its dark tile to `assets/icon/`: `icon.png` and `icon.ico` (full-bleed; the ICO is embedded in the Windows executable by
`build.rs`), and `icon-macos.png` and `AppIcon.icns` (on Apple's grid, for the Dock and the bundle). The Linux archive ships `icon.png` with
`layr.desktop`.

## Licences

Inter (`assets/Inter-*.ttf`) is under the SIL Open Font License, see `assets/Inter-LICENSE.txt`.
