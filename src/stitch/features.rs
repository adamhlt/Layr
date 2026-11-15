//! Feature points for stitching: FAST corners over a small scale pyramid, described with
//! oriented BRIEF (the ORB recipe), and brute-force matching of the binary descriptors.

use std::sync::OnceLock;

use image::RgbImage;

#[derive(Clone)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub d: Vec<f32>,
}

impl Gray {
    pub fn from_rgb(img: &RgbImage) -> Gray {
        let (w, h) = img.dimensions();
        let d = img.pixels().map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
        Gray { w: w as usize, h: h as usize, d }
    }

    pub fn blur(&self, sigma: f32) -> Gray {
        let r = (sigma * 3.0).ceil().max(1.0) as isize;
        let mut kernel: Vec<f32> = (-r..=r).map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp()).collect();
        let sum: f32 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0f32; self.d.len()];
        for y in 0..h {
            let row = &self.d[y * w..(y + 1) * w];
            for x in 0..w {
                let mut acc = 0.0;
                for (k, kv) in kernel.iter().enumerate() {
                    acc += kv * row[(x as isize + k as isize - r).clamp(0, w as isize - 1) as usize];
                }
                tmp[y * w + x] = acc;
            }
        }
        let mut out = vec![0f32; self.d.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for (k, kv) in kernel.iter().enumerate() {
                    acc += kv * tmp[(y as isize + k as isize - r).clamp(0, h as isize - 1) as usize * w + x];
                }
                out[y * w + x] = acc;
            }
        }
        Gray { w, h, d: out }
    }

    fn sample(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.w - 1) as f32);
        let y = y.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| self.d[y * self.w + x];
        (at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx) * (1.0 - fy) + (at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx) * fy
    }

    pub fn resize(&self, w: usize, h: usize) -> Gray {
        let (sx, sy) = (self.w as f32 / w as f32, self.h as f32 / h as f32);
        let mut d = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                d.push(self.sample((x as f32 + 0.5) * sx - 0.5, (y as f32 + 0.5) * sy - 0.5));
            }
        }
        Gray { w, h, d }
    }
}

#[derive(Clone, Copy)]
pub struct Feature {
    pub x: f32,
    pub y: f32,
    pub desc: [u64; 4],
}

const LEVELS: usize = 3;
const SCALE: f32 = 1.5;
/// Descriptors need this much image around their point.
const MARGIN: usize = 16;
const CIRCLE: [(i32, i32); 16] = [
    (0, -3),
    (1, -3),
    (2, -2),
    (3, -1),
    (3, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 3),
    (-1, 3),
    (-2, 2),
    (-3, 1),
    (-3, 0),
    (-3, -1),
    (-2, -2),
    (-1, -3),
];

/// Pairs of sample offsets, in a disc of radius 13, from a fixed pseudo-random sequence.
fn pattern() -> &'static [[f32; 4]; 256] {
    static P: OnceLock<[[f32; 4]; 256]> = OnceLock::new();
    P.get_or_init(|| {
        let mut state = 0x1234_5678_9ABC_DEF1u64;
        let mut rnd = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f32) / (1u64 << 31) as f32
        };
        let mut gauss = move || {
            // Box-Muller, with a spread of a fifth of the 31-pixel patch.
            let (a, b) = (rnd().max(1e-6), rnd());
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos() * 6.2
        };
        std::array::from_fn(|_| {
            let mut p = [0.0; 4];
            for v in &mut p {
                *v = gauss().clamp(-12.0, 12.0);
            }
            p
        })
    })
}

fn std_dev(d: &[f32]) -> f32 {
    let n = d.len().max(1) as f32;
    let mean = d.iter().sum::<f32>() / n;
    (d.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n).sqrt()
}

/// How strongly a contiguous arc of 9 circle pixels is all brighter or all darker than the
/// centre by more than `t`; 0 when none is.
fn arc_score(diffs: &[f32; 16], t: f32) -> f32 {
    for sign in [1.0f32, -1.0] {
        let (mut run, mut best) = (0, 0);
        for k in 0..32 {
            if diffs[k % 16] * sign > t {
                run += 1;
                best = best.max(run);
            } else {
                run = 0;
            }
        }
        if best >= 9 {
            return diffs.iter().map(|d| (d * sign - t).max(0.0)).sum();
        }
    }
    0.0
}

/// FAST-9 corners surviving non-maximum suppression, as `(x, y, score)`.
fn corners(g: &Gray, t: f32) -> Vec<(usize, usize, f32)> {
    let (w, h) = (g.w, g.h);
    if w < 2 * MARGIN + 1 || h < 2 * MARGIN + 1 {
        return Vec::new();
    }
    let mut score = vec![0f32; w * h];
    for y in MARGIN..h - MARGIN {
        for x in MARGIN..w - MARGIN {
            let c = g.d[y * w + x];
            let mut diffs = [0f32; 16];
            for (k, (dx, dy)) in CIRCLE.iter().enumerate() {
                diffs[k] = g.d[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize] - c;
            }
            let bright = [0, 4, 8, 12].iter().filter(|&&k| diffs[k] > t).count();
            let dark = [0, 4, 8, 12].iter().filter(|&&k| diffs[k] < -t).count();
            if bright >= 2 || dark >= 2 {
                score[y * w + x] = arc_score(&diffs, t);
            }
        }
    }
    let mut out = Vec::new();
    for y in MARGIN..h - MARGIN {
        for x in MARGIN..w - MARGIN {
            let s = score[y * w + x];
            if s > 0.0
                && (-1i32..=1).all(|dy| {
                    (-1i32..=1).all(|dx| (dx == 0 && dy == 0) || score[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize] < s)
                })
            {
                out.push((x, y, s));
            }
        }
    }
    out
}

/// Up to `max` features of the image, spread over it, with positions in its own pixels.
pub fn detect(base: &Gray, max: usize) -> Vec<Feature> {
    let t = (0.25 * std_dev(&base.d)).clamp(5.0, 25.0);
    let mut levels = vec![base.clone()];
    while levels.len() < LEVELS {
        let prev = levels.last().unwrap();
        let (w, h) = ((prev.w as f32 / SCALE) as usize, (prev.h as f32 / SCALE) as usize);
        if w < 4 * MARGIN || h < 4 * MARGIN {
            break;
        }
        let next = prev.blur(0.8).resize(w, h);
        levels.push(next);
    }
    struct Cand {
        level: usize,
        x: usize,
        y: usize,
        score: f32,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for (level, g) in levels.iter().enumerate() {
        cands.extend(corners(g, t).into_iter().map(|(x, y, score)| Cand { level, x, y, score }));
    }
    cands.sort_by(|a, b| b.score.total_cmp(&a.score));
    // Best first, but no more than a few per cell, so the points cover the whole image.
    const CELL: f32 = 32.0;
    let (cw, ch) = ((base.w as f32 / CELL).ceil() as usize, (base.h as f32 / CELL).ceil() as usize);
    let cap = ((max as f32 / (cw * ch).max(1) as f32) * 2.0).ceil().max(2.0) as usize;
    let mut count = vec![0usize; cw * ch];
    let mut kept: Vec<&Cand> = Vec::new();
    for c in &cands {
        let f = SCALE.powi(c.level as i32);
        let cell = ((c.y as f32 * f / CELL) as usize).min(ch - 1) * cw + ((c.x as f32 * f / CELL) as usize).min(cw - 1);
        if count[cell] < cap {
            count[cell] += 1;
            kept.push(c);
            if kept.len() >= max {
                break;
            }
        }
    }
    let smooth: Vec<Gray> = levels.iter().map(|g| g.blur(2.0)).collect();
    let pat = pattern();
    let mut out = Vec::with_capacity(kept.len());
    for c in kept {
        let g = &smooth[c.level];
        // Orientation: the direction from the patch's centre to its intensity centroid.
        let (mut m10, mut m01) = (0f32, 0f32);
        for dy in -9i32..=9 {
            for dx in -9i32..=9 {
                if dx * dx + dy * dy <= 81 {
                    let v = g.d[(c.y as i32 + dy) as usize * g.w + (c.x as i32 + dx) as usize];
                    m10 += dx as f32 * v;
                    m01 += dy as f32 * v;
                }
            }
        }
        let (s, co) = m01.atan2(m10).sin_cos();
        let at = |ox: f32, oy: f32| -> f32 {
            let x = (c.x as f32 + co * ox - s * oy).round().clamp(0.0, (g.w - 1) as f32) as usize;
            let y = (c.y as f32 + s * ox + co * oy).round().clamp(0.0, (g.h - 1) as f32) as usize;
            g.d[y * g.w + x]
        };
        let mut desc = [0u64; 4];
        for (i, p) in pat.iter().enumerate() {
            if at(p[0], p[1]) < at(p[2], p[3]) {
                desc[i / 64] |= 1 << (i % 64);
            }
        }
        let f = SCALE.powi(c.level as i32);
        out.push(Feature { x: c.x as f32 * f, y: c.y as f32 * f, desc });
    }
    out
}

fn distance(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (a[0] ^ b[0]).count_ones() + (a[1] ^ b[1]).count_ones() + (a[2] ^ b[2]).count_ones() + (a[3] ^ b[3]).count_ones()
}

/// For each of `a`: its nearest of `b`, with the distances to it and to the runner-up.
fn nearest(a: &[Feature], b: &[Feature]) -> Vec<(usize, u32, u32)> {
    a.iter()
        .map(|fa| {
            let (mut best, mut d1, mut d2) = (0, u32::MAX, u32::MAX);
            for (j, fb) in b.iter().enumerate() {
                let d = distance(&fa.desc, &fb.desc);
                if d < d1 {
                    (d2, d1, best) = (d1, d, j);
                } else if d < d2 {
                    d2 = d;
                }
            }
            (best, d1, d2)
        })
        .collect()
}

/// Index pairs `(i in a, j in b)` that are each other's nearest neighbour and clearly closer
/// than the second nearest.
pub fn match_features(a: &[Feature], b: &[Feature]) -> Vec<(usize, usize)> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let ab = nearest(a, b);
    let ba = nearest(b, a);
    ab.iter().enumerate().filter(|&(i, &(j, d1, d2))| (d1 as f32) < 0.8 * d2 as f32 && ba[j].0 == i).map(|(i, &(j, _, _))| (i, j)).collect()
}
