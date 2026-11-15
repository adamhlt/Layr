//! Drawing the mosaic: each image is placed in the common frame and shrunk to the size it has
//! there, its brightness is matched to its neighbours', and the result is blended band by band
//! (see `multiband`).

#![allow(clippy::needless_range_loop)] // index loops read better in the matrix code

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use image::{Rgb, RgbImage, RgbaImage};

use super::Progress;
use super::geom::{self, H, P};
use super::multiband::{self, CORE, Source};

pub struct Placed {
    pub img: RgbImage,
    /// Pixel index of the image → pixel index of the common frame.
    pub h: H,
}

fn corners(w: u32, h: u32) -> [P; 4] {
    let (w, h) = (w as f64 - 0.5, h as f64 - 0.5);
    [(-0.5, -0.5), (w, -0.5), (w, h), (-0.5, h)]
}

/// The common frame's extent, `[min x, min y, max x, max y]`, or none if an image folds over.
pub fn bounds(items: &[(u32, u32, H)]) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (w, h, m) in items {
        for (x, y) in corners(*w, *h) {
            if geom::depth(m, x, y) <= 1e-9 {
                return None;
            }
            let (u, v) = geom::apply(m, x, y)?;
            b = [b[0].min(u), b[1].min(v), b[2].max(u), b[3].max(v)];
        }
    }
    b.iter().all(|v| v.is_finite()).then_some(b)
}

/// Half the image's size, averaging 2×2 blocks.
fn halve(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let (nw, nh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let mut out = RgbImage::new(nw, nh);
    for y in 0..nh {
        for x in 0..nw {
            let mut acc = [0u32; 3];
            let mut n = 0;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = (2 * x + dx, 2 * y + dy);
                if sx < w && sy < h {
                    let p = img.get_pixel(sx, sy);
                    (0..3).for_each(|c| acc[c] += p[c] as u32);
                    n += 1;
                }
            }
            out.put_pixel(x, y, Rgb([0, 1, 2].map(|c| ((acc[c] + n / 2) / n) as u8)));
        }
    }
    out
}

fn bilinear(img: &RgbImage, u: f64, v: f64) -> [f32; 3] {
    let (w, h) = img.dimensions();
    let u = u.clamp(0.0, (w - 1) as f64);
    let v = v.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
    let (a, b, c, d) = (img.get_pixel(x0, y0), img.get_pixel(x1, y0), img.get_pixel(x0, y1), img.get_pixel(x1, y1));
    std::array::from_fn(|k| {
        let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
        let bottom = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

/// How far inside the image `(u, v)` is, as a share of the way to its middle: 0 at the edge,
/// 1 at the centre, none outside.
fn inside(img: &RgbImage, u: f64, v: f64) -> Option<f32> {
    let (w, h) = img.dimensions();
    let d = (u + 0.5).min(w as f64 - 0.5 - u).min(v + 0.5).min(h as f64 - 0.5 - v);
    (d > 0.0).then(|| (d / (0.5 * w.min(h) as f64)).min(1.0) as f32)
}

/// Solves `a x = b` by Gaussian elimination with pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        x[r] = (b[r] - (r + 1..n).map(|k| a[r][k] * x[k]).sum::<f64>()) / a[r][r];
    }
    Some(x)
}

/// Brightness gains that bring overlapping images to the same level (Brown & Lowe): each
/// overlap's mean brightness should agree once gained, with a pull of every gain towards 1.
/// `means[i][j]` is image `i`'s mean over its overlap with `j`, and `counts` the overlap's area.
fn gains(means: &[Vec<f64>], counts: &[Vec<f64>]) -> Vec<f64> {
    const SIGMA_N2: f64 = 10.0 * 10.0;
    const SIGMA_G2: f64 = 0.1 * 0.1;
    let n = means.len();
    let mut a = vec![vec![0.0; n]; n];
    let mut b = vec![0.0; n];
    for i in 0..n {
        for j in 0..n {
            let c = counts[i][j];
            if i == j || c <= 0.0 {
                continue;
            }
            a[i][i] += c * (means[i][j] * means[i][j] / SIGMA_N2 + 1.0 / SIGMA_G2);
            a[i][j] -= c * means[i][j] * means[j][i] / SIGMA_N2;
            b[i] += c / SIGMA_G2;
        }
        if a[i][i] == 0.0 {
            a[i][i] = 1.0;
            b[i] = 1.0;
        }
    }
    solve(a, b).map_or_else(|| vec![1.0; n], |g| g.into_iter().map(|v| v.clamp(0.5, 2.0)).collect())
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}

/// A warped, possibly shrunk image with the map from output pixels to its own.
struct Layer {
    img: RgbImage,
    /// Output pixel index → this image's pixel index.
    m: H,
    /// The rows and columns of the output it can touch: `[x0, y0, x1, y1]`.
    bbox: [usize; 4],
}

pub fn compose(placed: Vec<Placed>, max_side: u32, max_pixels: u64, progress: &Progress) -> Result<RgbaImage, String> {
    let items: Vec<_> = placed.iter().map(|p| (p.img.width(), p.img.height(), p.h)).collect();
    let [min_x, min_y, max_x, max_y] = bounds(&items).ok_or("the images can't lie on one flat plane")?;
    let (bw, bh) = (max_x - min_x, max_y - min_y);
    if !(bw >= 1.0 && bh >= 1.0) {
        return Err("the mosaic would be empty".into());
    }
    let k = (max_side as f64 / bw.max(bh)).min((max_pixels as f64 / (bw * bh)).sqrt()).min(1.0);
    let (ow, oh) = ((bw * k).ceil().max(1.0) as usize, (bh * k).ceil().max(1.0) as usize);
    let to_frame: H = [1.0 / k, 0.0, min_x, 0.0, 1.0 / k, min_y, 0.0, 0.0, 1.0];

    // Each image is shrunk to about the size it has in the output, so that sampling it
    // doesn't alias.
    let mut layers = Vec::new();
    for p in placed {
        let inv = geom::inv(&p.h).ok_or("an image has a degenerate placement")?;
        let mut m = geom::mul(&inv, &to_frame);
        let (w, h) = p.img.dimensions();
        let o = geom::apply(&geom::inv(&m).ok_or("an image has a degenerate placement")?, w as f64 / 2.0, h as f64 / 2.0)
            .ok_or("an image folds over")?;
        let at = |dx: f64, dy: f64| geom::apply(&m, o.0 + dx, o.1 + dy);
        let (c, ex, ey) = (at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0));
        let scale = match (c, ex, ey) {
            (Some(c), Some(ex), Some(ey)) => (((ex.0 - c.0) * (ey.1 - c.1) - (ex.1 - c.1) * (ey.0 - c.0)).abs()).sqrt(),
            _ => 1.0,
        };
        let mut img = p.img;
        let mut level = if scale.is_finite() && scale > 1.0 { scale.log2().floor() as u32 } else { 0 };
        while level > 0 && img.width() > 8 && img.height() > 8 {
            img = halve(&img);
            m = geom::mul(&geom::scaling(0.5), &m);
            level -= 1;
        }
        let back = geom::inv(&m).ok_or("an image has a degenerate placement")?;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for (cx, cy) in corners(img.width(), img.height()) {
            let (u, v) = geom::apply(&back, cx, cy).ok_or("an image folds over")?;
            (x0, y0, x1, y1) = (x0.min(u), y0.min(v), x1.max(u), y1.max(v));
        }
        let clampi = |v: f64, hi: usize| v.floor().clamp(0.0, hi as f64 - 1.0) as usize;
        layers.push(Layer { img, m, bbox: [clampi(x0 - 1.0, ow), clampi(y0 - 1.0, oh), clampi(x1 + 1.0, ow), clampi(y1 + 1.0, oh)] });
    }
    if progress.cancelled() {
        return Err("cancelled".into());
    }

    // Brightness: compare the images over their overlaps on a small copy of the mosaic.
    progress.set("Matching brightness", 0.66);
    let gain = brightness(&layers, ow, oh);
    if progress.cancelled() {
        return Err("cancelled".into());
    }

    progress.set("Blending", 0.7);
    let sources: Vec<Source> = layers.iter().zip(&gain).map(|(l, g)| Source { img: &l.img, m: l.m, gain: *g, bbox: l.bbox }).collect();
    let tiles: Vec<(usize, usize)> =
        (0..oh.div_ceil(CORE)).flat_map(|ty| (0..ow.div_ceil(CORE)).map(move |tx| (tx * CORE, ty * CORE))).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let out = Mutex::new(vec![0u8; ow * oh * 4]);
    // Each tile holds a few full-size planes: a handful of workers is enough to keep memory low.
    std::thread::scope(|s| {
        for _ in 0..threads().min(6) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= tiles.len() || progress.cancelled() {
                        return;
                    }
                    let (x0, y0) = tiles[i];
                    if let Some(px) = multiband::tile(&sources, x0, y0) {
                        let (w, h) = (CORE.min(ow - x0), CORE.min(oh - y0));
                        let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
                        for r in 0..h {
                            let o = ((y0 + r) * ow + x0) * 4;
                            out[o..o + w * 4].copy_from_slice(&px[r * CORE * 4..(r * CORE + w) * 4]);
                        }
                    }
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    progress.set("Blending", 0.7 + 0.3 * finished as f32 / tiles.len() as f32);
                }
            });
        }
    });
    if progress.cancelled() {
        return Err("cancelled".into());
    }
    let out = out.into_inner().unwrap_or_else(|e| e.into_inner());
    RgbaImage::from_raw(ow as u32, oh as u32, out).ok_or_else(|| "couldn't build the mosaic".to_string())
}

/// A gain per layer, from how bright each looks where it overlaps the others.
fn brightness(layers: &[Layer], ow: usize, oh: usize) -> Vec<f32> {
    let n = layers.len();
    // A copy of the mosaic about 400 pixels across.
    let ratio = (ow.max(oh) as f64 / 400.0).max(1.0);
    let (cw, ch) = ((ow as f64 / ratio).ceil() as usize, (oh as f64 / ratio).ceil() as usize);
    let shrink: H = [ratio, 0.0, 0.0, 0.0, ratio, 0.0, 0.0, 0.0, 1.0];
    let grays: Vec<Vec<f32>> = layers
        .iter()
        .map(|l| {
            let m = geom::mul(&l.m, &shrink);
            let mut g = vec![-1f32; cw * ch];
            for y in 0..ch {
                for x in 0..cw {
                    let (fx, fy) = (x as f64, y as f64);
                    let w = m[6] * fx + m[7] * fy + m[8];
                    if w <= 1e-12 {
                        continue;
                    }
                    let (u, v) = ((m[0] * fx + m[1] * fy + m[2]) / w, (m[3] * fx + m[4] * fy + m[5]) / w);
                    // Only the well-inside part of each image: its edge is vignetted.
                    if inside(&l.img, u, v).is_some_and(|t| t > 0.1) {
                        let c = bilinear(&l.img, u, v);
                        g[y * cw + x] = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
                    }
                }
            }
            g
        })
        .collect();
    let mut means = vec![vec![0.0; n]; n];
    let mut counts = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in i + 1..n {
            let (mut c, mut si, mut sj) = (0.0f64, 0.0f64, 0.0f64);
            for (a, b) in grays[i].iter().zip(&grays[j]) {
                if *a >= 0.0 && *b >= 0.0 {
                    c += 1.0;
                    si += *a as f64;
                    sj += *b as f64;
                }
            }
            if c >= 100.0 {
                (means[i][j], means[j][i]) = (si / c, sj / c);
                (counts[i][j], counts[j][i]) = (c, c);
            }
        }
    }
    gains(&means, &counts).into_iter().map(|g| g as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gains_reconcile_a_brighter_neighbour() {
        // Image 1 shows the same area 25 % brighter: the gains should close that gap.
        let means = vec![vec![0.0, 100.0], vec![125.0, 0.0]];
        let counts = vec![vec![0.0, 5000.0], vec![5000.0, 0.0]];
        let g = gains(&means, &counts);
        assert!((g[0] * 100.0 - g[1] * 125.0).abs() < 12.0, "{g:?}");
        assert!(g[0] > 1.0 && g[1] < 1.0);
    }

    #[test]
    fn unconnected_images_keep_their_brightness() {
        let g = gains(&vec![vec![0.0; 2]; 2], &vec![vec![0.0; 2]; 2]);
        assert_eq!(g, vec![1.0, 1.0]);
    }

    #[test]
    fn halving_averages_blocks() {
        let mut img = RgbImage::new(2, 2);
        for (i, p) in img.pixels_mut().enumerate() {
            *p = Rgb([(i * 10) as u8; 3]);
        }
        assert_eq!(halve(&img).get_pixel(0, 0)[0], 15);
    }
}
