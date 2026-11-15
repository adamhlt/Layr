<div align="center">

<img src="assets/icon/icon.png" width="112" alt="Layr">

# Layr

**Annotate any image, export it sharp.**

[![CI](https://github.com/adamhlt/Layr/actions/workflows/ci.yml/badge.svg)](https://github.com/adamhlt/Layr/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/adamhlt/Layr?color=16171C)](https://github.com/adamhlt/Layr/releases/latest)
![Platforms](https://img.shields.io/badge/Windows%20·%20macOS%20·%20Linux-16171C)
![Rust](https://img.shields.io/badge/Rust-Rune%20UI-16171C?logo=rust)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-16171C)](LICENSE)

</div>

---

Layr is a native desktop editor for drawing boxes, badges, wires, arrows, measures and captions
over photos, then exporting the result in full resolution. It's made for repair notes, technical
documentation and annotated screenshots.

<p align="center"><img src="docs/before-after.jpg" alt="An Intel 8086 die photo, before and after annotating it in Layr"></p>
<p align="center"><sub>Siemens SAB 8086 die (an Intel 8086 made under licence) by <a href="https://commons.wikimedia.org/wiki/File:Siemens_SAB_8086_die.JPG">Pauli Rautakorpi</a>, <a href="https://creativecommons.org/licenses/by/3.0/">CC BY 3.0</a>, resized and annotated. Block positions from Ken Shirriff's <a href="https://www.righto.com/2020/06/a-look-at-die-of-8086-processor.html">look at the 8086 die</a>.</sub></p>

## Features

**Annotate**
- Rectangles, hatched areas, wires, arrows, dots and multi-line captions
- Numbered badges that count up by themselves: circle, rounded, square, diamond or hexagon
- Move, resize and restyle anything, with undo, copy and paste

**Measure**
- Draw a line over something of known length and set the scale once
- Every measure then shows its real length, in µm, mm, cm, m or inches
- Boxes can show their real width and height

**Explain**
- One click builds a legend with a sample of each style used on the image
- Write what each style means; updating the legend keeps your text

**Style**
- Any installed font, bold, any size
- A colour editor that takes hex, `rgb()`, `hsl()` or HTML names, with opacity
- Random flat colours to grow a palette that's saved between sessions

**Export**
- PNG, JPEG (adjustable quality), lossless WebP, or PDF
- Scale from 25 % to 200 %, with or without the photo underneath
- In PDFs, shapes stay vectors and text can be selected, searched and copied

The canvas draws annotations as vectors, so they stay crisp at any zoom.

## Shortcuts

| | |
| --- | --- |
| `V` `R` `H` `N` `L` `A` `P` `T` `M` | Select, rectangle, hatch, badge, wire, arrow, dot, text, measure |
| `G` | Add a legend |
| `Ctrl+C` / `Ctrl+V` · `Ctrl+D` | Copy and paste · duplicate |
| `Ctrl+Z` / `Ctrl+Y` | Undo and redo |
| `Ctrl+O` · `Ctrl+E` | Open an image · export |
| `Ctrl+F` · wheel | Fit to window · zoom |
| `Space` + drag, middle or right button | Pan the view |
| `Shift` | Square boxes, lines at 45° |

## Download

Get the latest build for Windows, macOS or Linux from
[**Releases**](https://github.com/adamhlt/Layr/releases/latest).

On macOS the app isn't notarized: the first time, right-click it and choose **Open**.

## Build

```bash
cargo run --release
```

- **Toolchain**: `rust-toolchain.toml` pins the Rust version; rustup installs it on first use.
- **Linux**: needs `libxkbcommon-dev` and `libwayland-dev`.
- **Release**: pushing a `v*` tag builds all three platforms into one GitHub release.

## License

Layr is free software under the [GNU General Public License v3.0](LICENSE) or any later version.
The bundled Inter typeface is under the SIL Open Font License.

<div align="center"><sub>Built with <a href="https://github.com/anomalyco/rune">Rune</a></sub></div>
