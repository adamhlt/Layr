//! Plane geometry for stitching: 3×3 homographies, a direct linear transform and RANSAC.

#![allow(clippy::needless_range_loop)] // index loops read better in the matrix code

/// A homography, row-major, acting on `(x, y, 1)`.
pub type H = [f64; 9];
pub type P = (f64, f64);

pub const IDENTITY: H = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

pub fn mul(a: &H, b: &H) -> H {
    let mut r = [0.0; 9];
    for i in 0..3 {
        for j in 0..3 {
            r[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
        }
    }
    r
}

pub fn inv(h: &H) -> Option<H> {
    let [a, b, c, d, e, f, g, i, j] = *h;
    let det = a * (e * j - f * i) - b * (d * j - f * g) + c * (d * i - e * g);
    if !det.is_finite() || det.abs() < 1e-18 {
        return None;
    }
    let s = 1.0 / det;
    Some([
        (e * j - f * i) * s,
        (c * i - b * j) * s,
        (b * f - c * e) * s,
        (f * g - d * j) * s,
        (a * j - c * g) * s,
        (c * d - a * f) * s,
        (d * i - e * g) * s,
        (b * g - a * i) * s,
        (a * e - b * d) * s,
    ])
}

/// Scales `h` so that its last entry is 1 (when it isn't about 0).
pub fn normalized(h: &H) -> H {
    if h[8].abs() < 1e-12 {
        return *h;
    }
    let mut r = *h;
    for v in &mut r {
        *v /= h[8];
    }
    r
}

/// The image of `(x, y)`, or none when it goes to infinity.
pub fn apply(h: &H, x: f64, y: f64) -> Option<P> {
    let w = h[6] * x + h[7] * y + h[8];
    if w.abs() < 1e-12 {
        return None;
    }
    let (u, v) = ((h[0] * x + h[1] * y + h[2]) / w, (h[3] * x + h[4] * y + h[5]) / w);
    (u.is_finite() && v.is_finite()).then_some((u, v))
}

/// The depth `w` of `(x, y)`: positive for points in front of the plane's camera.
pub fn depth(h: &H, x: f64, y: f64) -> f64 {
    h[6] * x + h[7] * y + h[8]
}

/// The affine map taking pixel index `x` of an image to `s·(x + ½) − ½`: how a resized copy's
/// pixels line up with the original's.
pub fn scaling(s: f64) -> H {
    let o = 0.5 * s - 0.5;
    [s, 0.0, o, 0.0, s, o, 0.0, 0.0, 1.0]
}

// ---- Direct linear transform ------------------------------------------------------------------

/// The eigenvector of the smallest eigenvalue of a symmetric 9×9 matrix (cyclic Jacobi).
fn smallest_eigenvector(mut a: [[f64; 9]; 9]) -> [f64; 9] {
    let mut v = [[0.0; 9]; 9];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..60 {
        let mut off = 0.0;
        for p in 0..9 {
            for q in p + 1..9 {
                off += a[p][q] * a[p][q];
            }
        }
        let diag: f64 = (0..9).map(|i| a[i][i] * a[i][i]).sum();
        if off <= 1e-26 * diag.max(1e-300) {
            break;
        }
        for p in 0..8 {
            for q in p + 1..9 {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for row in a.iter_mut() {
                    let (x, y) = (row[p], row[q]);
                    row[p] = c * x - s * y;
                    row[q] = s * x + c * y;
                }
                for k in 0..9 {
                    let (x, y) = (a[p][k], a[q][k]);
                    a[p][k] = c * x - s * y;
                    a[q][k] = s * x + c * y;
                }
                for row in v.iter_mut() {
                    let (x, y) = (row[p], row[q]);
                    row[p] = c * x - s * y;
                    row[q] = s * x + c * y;
                }
            }
        }
    }
    let k = (0..9).min_by(|&i, &j| a[i][i].total_cmp(&a[j][j])).unwrap_or(0);
    std::array::from_fn(|i| v[i][k])
}

/// Hartley's normalisation: centre at the origin, mean distance √2.
fn normalizer(points: impl Iterator<Item = P> + Clone) -> H {
    let n = points.clone().count() as f64;
    let (sx, sy) = points.clone().fold((0.0, 0.0), |(x, y), p| (x + p.0, y + p.1));
    let (cx, cy) = (sx / n, sy / n);
    let mean = points.map(|p| ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()).sum::<f64>() / n;
    let s = if mean > 1e-12 { std::f64::consts::SQRT_2 / mean } else { 1.0 };
    [s, 0.0, -s * cx, 0.0, s, -s * cy, 0.0, 0.0, 1.0]
}

/// The homography taking each `.0` to its `.1` that fits the pairs best (least squares on the
/// algebraic error). Needs four pairs, no three on a line.
pub fn dlt(pairs: &[(P, P)]) -> Option<H> {
    if pairs.len() < 4 {
        return None;
    }
    let t1 = normalizer(pairs.iter().map(|p| p.0));
    let t2 = normalizer(pairs.iter().map(|p| p.1));
    let mut m = [[0.0; 9]; 9];
    for &(a, b) in pairs {
        let (x, y) = apply(&t1, a.0, a.1)?;
        let (u, v) = apply(&t2, b.0, b.1)?;
        let rows = [[-x, -y, -1.0, 0.0, 0.0, 0.0, u * x, u * y, u], [0.0, 0.0, 0.0, -x, -y, -1.0, v * x, v * y, v]];
        for r in rows {
            for i in 0..9 {
                for j in 0..9 {
                    m[i][j] += r[i] * r[j];
                }
            }
        }
    }
    let h = mul(&mul(&inv(&t2)?, &smallest_eigenvector(m)), &t1);
    let h = normalized(&h);
    (h[8].abs() > 1e-12 && h.iter().all(|v| v.is_finite())).then_some(h)
}

// ---- RANSAC -----------------------------------------------------------------------------------

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() >> 11) as usize % n.max(1)
    }
}

fn error(h: &H, a: P, b: P) -> f64 {
    apply(h, a.0, a.1).map_or(f64::INFINITY, |p| ((p.0 - b.0).powi(2) + (p.1 - b.1).powi(2)).sqrt())
}

fn collinear(p: [P; 3]) -> bool {
    ((p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[1].1 - p[0].1) * (p[2].0 - p[0].0)).abs() < 4.0
}

fn degenerate(s: &[(P, P); 4]) -> bool {
    (0..4).any(|skip| {
        let a: Vec<P> = (0..4).filter(|&i| i != skip).map(|i| s[i].0).collect();
        let b: Vec<P> = (0..4).filter(|&i| i != skip).map(|i| s[i].1).collect();
        collinear([a[0], a[1], a[2]]) || collinear([b[0], b[1], b[2]])
    })
}

/// A homography taking `.0` to `.1` for most of `pairs`, with the indices of those it fits to
/// within `thr` pixels.
pub fn ransac(pairs: &[(P, P)], thr: f64, max_iter: usize, rng: &mut Rng) -> Option<(H, Vec<usize>)> {
    let n = pairs.len();
    if n < 4 {
        return None;
    }
    let inliers = |h: &H| -> Vec<usize> { (0..n).filter(|&i| error(h, pairs[i].0, pairs[i].1) < thr).collect() };
    let mut best: Option<(H, Vec<usize>)> = None;
    let mut needed = max_iter as f64;
    let mut iter = 0.0;
    while iter < needed.min(max_iter as f64) {
        iter += 1.0;
        let mut idx = [0usize; 4];
        for k in 0..4 {
            loop {
                let c = rng.below(n);
                if !idx[..k].contains(&c) {
                    idx[k] = c;
                    break;
                }
            }
        }
        let sample = idx.map(|i| pairs[i]);
        if degenerate(&sample) {
            continue;
        }
        let Some(h) = dlt(&sample) else { continue };
        let found = inliers(&h);
        if best.as_ref().is_none_or(|b| found.len() > b.1.len()) {
            let ratio = found.len() as f64 / n as f64;
            // Enough rounds to have drawn an all-inlier sample with 99.5 % confidence.
            needed = if ratio >= 0.999 { 1.0 } else { (0.005f64.ln() / (1.0 - ratio.powi(4)).max(1e-12).ln()).max(1.0) };
            best = Some((h, found));
        }
    }
    let (mut h, mut found) = best?;
    // Refit on every inlier, which also pulls in points the four-point sample missed.
    for _ in 0..4 {
        let subset: Vec<_> = found.iter().map(|&i| pairs[i]).collect();
        let Some(refit) = dlt(&subset) else { break };
        let next = inliers(&refit);
        if next.len() < found.len() {
            break;
        }
        let same = next.len() == found.len();
        (h, found) = (refit, next);
        if same {
            break;
        }
    }
    Some((h, found))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P, b: P, tol: f64) -> bool {
        (a.0 - b.0).abs() < tol && (a.1 - b.1).abs() < tol
    }

    const TRUTH: H = [0.98, -0.05, 120.0, 0.04, 1.02, -35.0, 2e-5, -1e-5, 1.0];

    fn grid() -> Vec<P> {
        (0..10)
            .flat_map(|i| (0..8).map(move |j| (60.0 * i as f64 + 7.0 * (j % 3) as f64, 55.0 * j as f64 + 5.0 * (i % 4) as f64)))
            .collect()
    }

    #[test]
    fn inverse_undoes_the_homography() {
        let i = mul(&TRUTH, &inv(&TRUTH).unwrap());
        for (a, b) in i.iter().zip(IDENTITY) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn dlt_recovers_a_homography() {
        let pairs: Vec<_> = grid().into_iter().map(|p| (p, apply(&TRUTH, p.0, p.1).unwrap())).collect();
        let h = dlt(&pairs).unwrap();
        for (a, b) in &pairs {
            assert!(close(apply(&h, a.0, a.1).unwrap(), *b, 1e-6));
        }
    }

    #[test]
    fn ransac_ignores_outliers_and_noise() {
        let mut rng = Rng::new(7);
        let mut pairs: Vec<_> = grid()
            .into_iter()
            .map(|p| {
                let q = apply(&TRUTH, p.0, p.1).unwrap();
                let n = |r: &mut Rng| (r.below(1000) as f64 / 1000.0 - 0.5) * 0.8;
                (p, (q.0 + n(&mut rng), q.1 + n(&mut rng)))
            })
            .collect();
        let good = pairs.len();
        for k in 0..40 {
            pairs.push(((rng.below(600) as f64, rng.below(450) as f64), (rng.below(600) as f64 + k as f64, rng.below(450) as f64)));
        }
        let (h, found) = ransac(&pairs, 2.0, 1000, &mut Rng::new(3)).unwrap();
        assert!(found.len() >= good - 2 && found.iter().filter(|&&i| i < good).count() >= good - 2);
        assert!(close(apply(&h, 300.0, 200.0).unwrap(), apply(&TRUTH, 300.0, 200.0).unwrap(), 1.0));
    }
}
