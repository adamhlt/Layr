//! Multi-band blending (Burt & Adelson), the way OpenPano and Brown & Lowe blend panoramas.
//!
//! Every output pixel belongs to one image: the one it sits most centrally in (OpenPano's
//! weight, `(½ − |x − ½|)(½ − |y − ½|)` in the image's own coordinates). Fine detail is taken
//! from that image alone, so a subject that moved between shots appears once instead of as a
//! ghost; coarser and coarser bands are mixed over wider and wider strips around the seams, so
//! differences in brightness fade out instead of showing as an edge.
//!
//! The mosaic is processed in square tiles with a margin wide enough for the coarsest band,
//! which keeps memory bounded whatever the mosaic's size.

#![allow(clippy::needless_range_loop)] // index loops read better in the filter code

use image::RgbImage;

use super::geom::H;

/// The number of bands below full resolution: the coarsest is 1/32 of the size.
pub const BANDS: usize = 5;
/// Pixels of context around a tile; covers the coarsest band's reach (≈ 4 · 2⁵).
pub const MARGIN: usize = 192;
/// The part of a tile that is kept. `CORE + 2·MARGIN` is a multiple of `2^BANDS`.
pub const CORE: usize = 640;
const SIZE: usize = CORE + 2 * MARGIN;
const NONE: u16 = u16::MAX;

/// An image placed in the output: `m` maps output pixels to its pixels.
pub struct Source<'a> {
    pub img: &'a RgbImage,
    pub m: H,
    pub gain: f32,
    /// The output pixels it can cover: `[x0, y0, x1, y1]`, inclusive.
    pub bbox: [usize; 4],
}

/// A `w × h` image of `c` interleaved float channels.
#[derive(Clone)]
struct Plane {
    w: usize,
    h: usize,
    c: usize,
    d: Vec<f32>,
}

const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

impl Plane {
    fn new(w: usize, h: usize, c: usize) -> Plane {
        Plane { w, h, c, d: vec![0.0; w * h * c] }
    }

    /// Blurs with the 5-tap binomial kernel and keeps every other pixel.
    fn reduce(&self) -> Plane {
        let (w, h, c) = (self.w, self.h, self.c);
        let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
        let mut tmp = Plane::new(nw, h, c);
        for y in 0..h {
            for ox in 0..nw {
                for (k, kv) in K.iter().enumerate() {
                    let x = (2 * ox as isize + k as isize - 2).clamp(0, w as isize - 1) as usize;
                    for ch in 0..c {
                        tmp.d[(y * nw + ox) * c + ch] += kv * self.d[(y * w + x) * c + ch];
                    }
                }
            }
        }
        let mut out = Plane::new(nw, nh, c);
        for oy in 0..nh {
            for (k, kv) in K.iter().enumerate() {
                let y = (2 * oy as isize + k as isize - 2).clamp(0, h as isize - 1) as usize;
                let (src, dst) = (&tmp.d[y * nw * c..(y + 1) * nw * c], &mut out.d[oy * nw * c..(oy + 1) * nw * c]);
                for (d, s) in dst.iter_mut().zip(src) {
                    *d += kv * s;
                }
            }
        }
        out
    }

    /// Doubles the size to `w × h`, interpolating with the same kernel.
    fn expand(&self, w: usize, h: usize) -> Plane {
        let (cw, ch_, c) = (self.w, self.h, self.c);
        let at = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
        // For an even output index the coarse taps are j−1, j, j+1 weighted 1, 6, 1; for an
        // odd one, j and j+1 weighted 4, 4 (all over 8).
        let taps = |x: usize, n: usize| -> [(usize, f32); 3] {
            let j = (x / 2) as isize;
            if x.is_multiple_of(2) {
                [(at(j - 1, n), 0.125), (at(j, n), 0.75), (at(j + 1, n), 0.125)]
            } else {
                [(at(j, n), 0.5), (at(j + 1, n), 0.5), (at(j, n), 0.0)]
            }
        };
        let mut tmp = Plane::new(w, ch_, c);
        for y in 0..ch_ {
            for x in 0..w {
                for (j, kv) in taps(x, cw) {
                    for k in 0..c {
                        tmp.d[(y * w + x) * c + k] += kv * self.d[(y * cw + j) * c + k];
                    }
                }
            }
        }
        let mut out = Plane::new(w, h, c);
        for y in 0..h {
            for (j, kv) in taps(y, ch_) {
                let (src, dst) = (&tmp.d[j * w * c..(j + 1) * w * c], &mut out.d[y * w * c..(y + 1) * w * c]);
                for (d, s) in dst.iter_mut().zip(src) {
                    *d += kv * s;
                }
            }
        }
        out
    }
}

/// Where output pixel `(x, y)` falls in the source, and how centrally; none outside it.
fn locate(s: &Source, x: f64, y: f64) -> Option<(f64, f64, f32)> {
    let m = &s.m;
    let w = m[6] * x + m[7] * y + m[8];
    if w <= 1e-12 {
        return None;
    }
    let (u, v) = ((m[0] * x + m[1] * y + m[2]) / w, (m[3] * x + m[4] * y + m[5]) / w);
    let (iw, ih) = (s.img.width() as f64, s.img.height() as f64);
    let (nu, nv) = ((u + 0.5) / iw, (v + 0.5) / ih);
    if !(0.0..1.0).contains(&nu) || !(0.0..1.0).contains(&nv) {
        return None;
    }
    Some((u, v, ((0.5 - (nu - 0.5).abs()) * (0.5 - (nv - 0.5).abs())) as f32 + 1e-6))
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

/// A tile's pixels, `CORE × CORE` from output pixel `(x0, y0)`: RGBA, transparent where no
/// image reaches. None when the tile is empty.
pub fn tile(sources: &[Source], x0: usize, y0: usize) -> Option<Vec<u8>> {
    // The tile with its margin, in output pixels (it may start left of or above the output).
    let (px, py) = (x0 as isize - MARGIN as isize, y0 as isize - MARGIN as isize);
    let span = |s: &Source| -> Option<[usize; 4]> {
        let [bx0, by0, bx1, by1] = s.bbox.map(|v| v as isize);
        let (ax0, ay0) = (bx0.max(px), by0.max(py));
        let (ax1, ay1) = (bx1.min(px + SIZE as isize - 1), by1.min(py + SIZE as isize - 1));
        (ax0 <= ax1 && ay0 <= ay1).then(|| [(ax0 - px) as usize, (ay0 - py) as usize, (ax1 - px) as usize, (ay1 - py) as usize])
    };

    // Which image each pixel belongs to.
    let mut owner = vec![NONE; SIZE * SIZE];
    let mut best = vec![0f32; SIZE * SIZE];
    let spans: Vec<Option<[usize; 4]>> = sources.iter().map(span).collect();
    for (k, (s, sp)) in sources.iter().zip(&spans).enumerate() {
        let Some([sx0, sy0, sx1, sy1]) = *sp else { continue };
        for ty in sy0..=sy1 {
            for tx in sx0..=sx1 {
                let (x, y) = ((px + tx as isize) as f64, (py + ty as isize) as f64);
                if let Some((_, _, w)) = locate(s, x, y)
                    && w > best[ty * SIZE + tx]
                {
                    best[ty * SIZE + tx] = w;
                    owner[ty * SIZE + tx] = k as u16;
                }
            }
        }
    }
    let core_owned = (MARGIN..MARGIN + CORE).any(|ty| owner[ty * SIZE + MARGIN..ty * SIZE + MARGIN + CORE].iter().any(|&o| o != NONE));
    if !core_owned {
        return None;
    }

    let mut sum: Vec<Plane> = (0..=BANDS).map(|l| Plane::new(SIZE >> l, SIZE >> l, 3)).collect();
    let mut weight: Vec<Plane> = (0..=BANDS).map(|l| Plane::new(SIZE >> l, SIZE >> l, 1)).collect();
    for (k, (s, sp)) in sources.iter().zip(&spans).enumerate() {
        let Some([sx0, sy0, sx1, sy1]) = *sp else { continue };
        if !owner.contains(&(k as u16)) {
            continue;
        }
        // The image, premultiplied by where it exists.
        let mut pre = Plane::new(SIZE, SIZE, 4);
        for ty in sy0..=sy1 {
            for tx in sx0..=sx1 {
                let (x, y) = ((px + tx as isize) as f64, (py + ty as isize) as f64);
                if let Some((u, v, _)) = locate(s, x, y) {
                    let c = bilinear(s.img, u, v);
                    let o = (ty * SIZE + tx) * 4;
                    pre.d[o..o + 3].copy_from_slice(&c.map(|v| v * s.gain));
                    pre.d[o + 3] = 1.0;
                }
            }
        }
        let filled = fill(pre);
        // Laplacian pyramid of the image, Gaussian pyramid of where it is used.
        let mut gauss = vec![filled];
        let mut mask = vec![Plane { w: SIZE, h: SIZE, c: 1, d: owner.iter().map(|&o| if o == k as u16 { 1.0 } else { 0.0 }).collect() }];
        for l in 0..BANDS {
            gauss.push(gauss[l].reduce());
            mask.push(mask[l].reduce());
        }
        for l in 0..=BANDS {
            let band = if l == BANDS {
                gauss[l].clone()
            } else {
                let up = gauss[l + 1].expand(gauss[l].w, gauss[l].h);
                let mut b = gauss[l].clone();
                b.d.iter_mut().zip(&up.d).for_each(|(v, u)| *v -= u);
                b
            };
            let (acc, wacc, m) = (&mut sum[l], &mut weight[l], &mask[l]);
            for i in 0..m.d.len() {
                let mw = m.d[i];
                if mw > 0.0 {
                    for ch in 0..3 {
                        acc.d[i * 3 + ch] += band.d[i * 3 + ch] * mw;
                    }
                    wacc.d[i] += mw;
                }
            }
        }
    }
    for (acc, w) in sum.iter_mut().zip(&weight) {
        for (i, &wv) in w.d.iter().enumerate() {
            let f = if wv > 1e-6 { 1.0 / wv } else { 0.0 };
            for ch in 0..3 {
                acc.d[i * 3 + ch] *= f;
            }
        }
    }
    // Collapse the pyramid.
    let mut img = sum.pop().unwrap_or_else(|| Plane::new(1, 1, 3));
    while let Some(band) = sum.pop() {
        let mut up = img.expand(band.w, band.h);
        up.d.iter_mut().zip(&band.d).for_each(|(v, b)| *v += b);
        img = up;
    }
    let mut out = vec![0u8; CORE * CORE * 4];
    for cy in 0..CORE {
        for cx in 0..CORE {
            let t = (cy + MARGIN) * SIZE + cx + MARGIN;
            if owner[t] == NONE {
                continue;
            }
            let o = (cy * CORE + cx) * 4;
            for ch in 0..3 {
                out[o + ch] = img.d[t * 3 + ch].round().clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = 255;
        }
    }
    Some(out)
}

/// Extends an image beyond its edges with its own blurred colours (push-pull), so the pyramid
/// sees no artificial black border there: that border is what would bleed a dark halo along
/// the seams. In: RGB premultiplied by coverage, and coverage. Out: RGB.
fn fill(pre: Plane) -> Plane {
    let mut levels = vec![pre];
    while levels.last().is_some_and(|p| p.w > 1 || p.h > 1) {
        let next = levels.last().unwrap().reduce();
        levels.push(next);
    }
    let unpremultiply = |p: &Plane| -> Plane {
        let mut out = Plane::new(p.w, p.h, 3);
        for i in 0..p.w * p.h {
            let a = p.d[i * 4 + 3];
            if a > 1e-6 {
                for ch in 0..3 {
                    out.d[i * 3 + ch] = p.d[i * 4 + ch] / a;
                }
            }
        }
        out
    };
    let mut color = unpremultiply(levels.last().unwrap());
    for p in levels.iter().rev().skip(1) {
        let up = color.expand(p.w, p.h);
        let mut next = Plane::new(p.w, p.h, 3);
        for i in 0..p.w * p.h {
            let a = p.d[i * 4 + 3].min(1.0);
            for ch in 0..3 {
                // The image over what the coarser level guessed.
                next.d[i * 3 + ch] = p.d[i * 4 + ch] + (1.0 - a) * up.d[i * 3 + ch];
            }
        }
        color = next;
    }
    color
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn flat(w: u32, h: u32, v: u8) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb([v; 3]))
    }

    #[test]
    fn reduce_and_expand_keep_a_constant() {
        let p = Plane { w: 13, h: 9, c: 2, d: vec![3.0; 13 * 9 * 2] };
        let r = p.reduce();
        assert_eq!((r.w, r.h), (7, 5));
        assert!(r.d.iter().all(|v| (v - 3.0).abs() < 1e-5));
        let e = r.expand(13, 9);
        assert!(e.d.iter().all(|v| (v - 3.0).abs() < 1e-5));
    }

    #[test]
    fn fill_extends_colours_past_the_edge() {
        let mut p = Plane::new(64, 64, 4);
        for y in 0..64 {
            for x in 0..20 {
                let o = (y * 64 + x) * 4;
                p.d[o..o + 4].copy_from_slice(&[200.0, 100.0, 50.0, 1.0]);
            }
        }
        let f = fill(p);
        // Far from the image, the colour is still the image's, not black.
        let o = (32 * 64 + 60) * 3;
        assert!((f.d[o] - 200.0).abs() < 1.0 && (f.d[o + 1] - 100.0).abs() < 1.0, "{:?}", &f.d[o..o + 3]);
    }

    /// Two flat images side by side, overlapping by 200 px: the result must go from one value
    /// to the other without overshoot, and stay exact far from the seam and at the edges.
    #[test]
    fn a_seam_fades_without_halo() {
        let (a, b) = (flat(500, 400, 100), flat(500, 400, 160));
        let shift = |dx: f64| [1.0, 0.0, -dx, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        let sources = [
            Source { img: &a, m: shift(0.0), gain: 1.0, bbox: [0, 0, 499, 399] },
            Source { img: &b, m: shift(300.0), gain: 1.0, bbox: [300, 0, 799, 399] },
        ];
        let t = tile(&sources, 0, 0).unwrap();
        let px = |x: usize, y: usize| t[(y * CORE + x) * 4..(y * CORE + x) * 4 + 4].to_vec();
        for y in [0, 200, 399] {
            assert_eq!(px(5, y), vec![100, 100, 100, 255], "left edge, row {y}");
            assert_eq!(px(630, y), vec![160, 160, 160, 255], "right side, row {y}");
            let mut last = 0;
            for x in 0..640 {
                let v = px(x, y)[0];
                assert!((100..=160).contains(&v), "overshoot {v} at {x},{y}");
                assert!(v >= last, "not monotone at {x},{y}");
                last = v;
            }
        }
        assert_eq!(px(5, 450)[3], 0, "below the images stays empty");
    }
}
