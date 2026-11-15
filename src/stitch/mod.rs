//! Stitching several overlapping photos into one large image.
//!
//! The photos need not be ordered, aligned or taken from the same spot on a grid: features
//! are found in each (FAST corners with oriented BRIEF descriptors), matched pairwise, and
//! checked with RANSAC, which gives a homography for every pair that overlaps. A reference
//! image is chosen, every other image is placed in its frame and the placements are refined
//! against all the overlaps. The images are then warped, brightness-matched and blended in
//! frequency bands with a seam between them, as OpenPano does.
//!
//! Homographies model a flat subject (a die, a board, a page, a wall) or a camera that only
//! turns; they stretch badly for a panorama wider than about 120°.

mod align;
mod blend;
mod features;
mod geom;
pub mod job;
mod multiband;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use image::imageops::FilterType;
use image::{ImageDecoder, RgbImage, RgbaImage};

use align::Edge;
use geom::{H, P};

/// Matching works on a copy this many pixels on its longest side.
const WORK_SIDE: u32 = 1600;
const FEATURES: usize = 2000;

pub struct Options {
    /// The longest side the mosaic may have.
    pub max_side: u32,
    /// The most pixels it may have.
    pub max_pixels: u64,
    /// Trims the mosaic to the largest rectangle the photos fill, losing the ragged outline.
    pub crop: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options { max_side: 8192, max_pixels: 8192 * 8192, crop: true }
    }
}

pub struct Mosaic {
    pub image: RgbaImage,
    pub used: usize,
    /// Images that overlapped nothing else and were left out.
    pub skipped: Vec<String>,
}

/// What the background job reports, and the way to stop it.
pub struct Progress {
    cancel: AtomicBool,
    frac: AtomicU32,
    stage: Mutex<String>,
}

impl Progress {
    pub fn new() -> Progress {
        Progress { cancel: AtomicBool::new(false), frac: AtomicU32::new(0), stage: Mutex::new(String::new()) }
    }

    pub fn set(&self, stage: &str, frac: f32) {
        self.frac.store(frac.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        let mut s = self.lock();
        if *s != stage {
            *s = stage.to_string();
        }
    }

    fn lock(&self) -> MutexGuard<'_, String> {
        self.stage.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn fraction(&self) -> f32 {
        f32::from_bits(self.frac.load(Ordering::Relaxed))
    }

    pub fn stage(&self) -> String {
        self.lock().clone()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

fn threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}

/// `f(0..n)` on every core, results in order.
fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads().min(n.max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let r = f(i);
                    out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().flatten().collect()
}

/// An image file as RGB, turned upright by its EXIF orientation.
pub fn load(path: &Path) -> Result<RgbImage, String> {
    let reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img.to_rgb8())
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

pub fn stitch_files(paths: &[PathBuf], opts: &Options, progress: &Progress) -> Result<Mosaic, String> {
    let mut images = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        progress.set("Reading the images", 0.1 * i as f32 / paths.len().max(1) as f32);
        if progress.cancelled() {
            return Err("cancelled".into());
        }
        images.push((name_of(p), load(p).map_err(|e| format!("{}: {e}", name_of(p)))?));
    }
    stitch(images, opts, progress)
}

/// How far the area of a quad is from `1` after the map: 0 when it folds over or collapses.
fn area_ratio(h: &H, w: u32, hh: u32) -> f64 {
    let c = [(0.0, 0.0), (w as f64, 0.0), (w as f64, hh as f64), (0.0, hh as f64)];
    let mut q = Vec::new();
    for (x, y) in c {
        if geom::depth(h, x, y) <= 1e-9 {
            return 0.0;
        }
        match geom::apply(h, x, y) {
            Some(p) => q.push(p),
            None => return 0.0,
        }
    }
    let area = |p: &[P]| (0..4).map(|i| p[i].0 * p[(i + 1) % 4].1 - p[(i + 1) % 4].0 * p[i].1).sum::<f64>() / 2.0;
    let before = area(&c);
    let after = area(&q);
    if after <= 0.0 { 0.0 } else { after / before }
}

struct Registration {
    /// Each image's pixel index → the reference image's, at full resolution.
    h: Vec<Option<H>>,
}

fn register(images: &[(String, RgbImage)], progress: &Progress) -> Result<Registration, String> {
    let n = images.len();
    // Work copies: grayscale, at most WORK_SIDE on a side.
    progress.set("Finding features", 0.1);
    let work: Vec<(f64, Vec<features::Feature>)> = par_map(n, |i| {
        let img = &images[i].1;
        let (w, h) = img.dimensions();
        let s = (WORK_SIDE as f64 / w.max(h) as f64).min(1.0);
        let gray = if s < 1.0 {
            let small = image::imageops::resize(
                img,
                (w as f64 * s).round().max(1.0) as u32,
                (h as f64 * s).round().max(1.0) as u32,
                FilterType::Triangle,
            );
            features::Gray::from_rgb(&small)
        } else {
            features::Gray::from_rgb(img)
        };
        // The scale really applied, after rounding the size.
        let s = gray.w as f64 / w as f64;
        (s, features::detect(&gray, FEATURES))
    });
    if progress.cancelled() {
        return Err("cancelled".into());
    }

    progress.set("Matching the images", 0.25);
    let pairs: Vec<(usize, usize)> = (0..n).flat_map(|i| (i + 1..n).map(move |j| (i, j))).collect();
    let done = AtomicUsize::new(0);
    let edges: Vec<Option<Edge>> = par_map(pairs.len(), |k| {
        let (i, j) = pairs[k];
        let edge = (|| {
            if progress.cancelled() {
                return None;
            }
            let matches = features::match_features(&work[i].1, &work[j].1);
            if matches.len() < 12 {
                return None;
            }
            let corr: Vec<(P, P)> = matches
                .iter()
                .map(|&(a, b)| {
                    let (fa, fb) = (&work[i].1[a], &work[j].1[b]);
                    ((fa.x as f64, fa.y as f64), (fb.x as f64, fb.y as f64))
                })
                .collect();
            let (h, inliers) = geom::ransac(&corr, 3.0, 1000, &mut geom::Rng::new((i * 7919 + j) as u64))?;
            // Brown & Lowe's test: the inliers must be clearly more than chance would give.
            if inliers.len() < 12 || (inliers.len() as f64) <= 5.9 + 0.22 * corr.len() as f64 {
                return None;
            }
            let (w, hh) = images[i].1.dimensions();
            let ratio = area_ratio(&h, (w as f64 * work[i].0) as u32, (hh as f64 * work[i].0) as u32);
            if !(0.1..=10.0).contains(&ratio) {
                return None;
            }
            Some(Edge { i, j, h, pairs: inliers.iter().map(|&k| corr[k]).collect() })
        })();
        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
        progress.set("Matching the images", 0.25 + 0.35 * finished as f32 / pairs.len() as f32);
        edge
    });
    if progress.cancelled() {
        return Err("cancelled".into());
    }
    let edges: Vec<Edge> = edges.into_iter().flatten().collect();

    progress.set("Aligning", 0.62);
    let layout = align::layout(n, &edges)
        .ok_or("No two images overlap enough to be joined. Use photos that share about a third of their content.")?;
    let s_ref = geom::inv(&geom::scaling(work[layout.reference].0)).ok_or("bad scale")?;
    let mut h: Vec<Option<H>> = vec![None; n];
    for (i, hi) in layout.h.iter().enumerate() {
        let Some(hi) = hi else { continue };
        let mut full = geom::normalized(&geom::mul(&geom::mul(&s_ref, hi), &geom::scaling(work[i].0)));
        let (w, hh) = images[i].1.dimensions();
        // Either sign describes the same map; the right one has the image in front.
        if geom::depth(&full, w as f64 / 2.0, hh as f64 / 2.0) < 0.0 {
            full.iter_mut().for_each(|v| *v = -*v);
        }
        h[i] = Some(full);
    }
    Ok(Registration { h })
}

pub fn stitch(mut images: Vec<(String, RgbImage)>, opts: &Options, progress: &Progress) -> Result<Mosaic, String> {
    if images.len() < 2 {
        return Err("Pick at least two images.".into());
    }
    let total: u64 = images.iter().map(|(_, i)| i.width() as u64 * i.height() as u64).sum();
    if total > 700_000_000 {
        return Err(format!("{} megapixels of photos is too much to hold in memory at once.", total / 1_000_000));
    }
    let reg = register(&images, progress)?;

    progress.set("Placing the images", 0.64);
    let mut placed = Vec::new();
    let mut skipped = Vec::new();
    for (i, (name, img)) in images.iter_mut().enumerate() {
        let (w, h) = img.dimensions();
        let ok = reg.h[i].filter(|m| blend::bounds(&[(w, h, *m)]).is_some());
        match ok {
            Some(m) => placed.push(blend::Placed { img: std::mem::take(img), h: m }),
            None => skipped.push(std::mem::take(name)),
        }
    }
    // A few images that fan out wildly are a failed match, not a mosaic.
    let items: Vec<_> = placed.iter().map(|p| (p.img.width(), p.img.height(), p.h)).collect();
    let [x0, y0, x1, y1] = blend::bounds(&items).ok_or("The images can't be laid on one flat plane.")?;
    let sum: f64 = items.iter().map(|(w, h, _)| *w as f64 * *h as f64).sum();
    if (x1 - x0) * (y1 - y0) > 60.0 * sum {
        return Err("The images don't line up into a flat mosaic (the view is too wide for this method).".into());
    }
    let used = placed.len();
    let mut image = blend::compose(placed, opts.max_side, opts.max_pixels, progress)?;
    if opts.crop {
        progress.set("Cropping", 1.0);
        let (x, y, w, h) = largest_filled_rect(&image).ok_or("The mosaic is empty.")?;
        image = image::imageops::crop_imm(&image, x, y, w, h).to_image();
    }
    progress.set("Done", 1.0);
    Ok(Mosaic { image, used, skipped })
}

/// The largest axis-aligned rectangle of opaque pixels, `(x, y, width, height)`: for each row,
/// the opaque run above every pixel is a histogram, and its largest rectangle is found with a
/// stack in one pass.
pub fn largest_filled_rect(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let mut heights = vec![0usize; w];
    let mut stack: Vec<usize> = Vec::with_capacity(w);
    let mut best = (0usize, 0usize, 0usize, 0usize, 0usize);
    for y in 0..h {
        for (x, hgt) in heights.iter_mut().enumerate() {
            *hgt = if raw[(y * w + x) * 4 + 3] == 255 { *hgt + 1 } else { 0 };
        }
        stack.clear();
        for x in 0..=w {
            let current = if x < w { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= current {
                    break;
                }
                stack.pop();
                let left = stack.last().map_or(0, |&l| l + 1);
                let (width, height) = (x - left, heights[top]);
                if width * height > best.0 {
                    best = (width * height, left, y + 1 - height, width, height);
                }
            }
            if x < w {
                stack.push(x);
            }
        }
    }
    let (area, x, y, width, height) = best;
    (area > 0).then_some((x as u32, y as u32, width as u32, height as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn hash(x: i32, y: i32, seed: u32) -> f32 {
        let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297A_2D39);
        h ^= h >> 15;
        (h & 0xFFFF) as f32 / 65535.0
    }

    fn noise(x: f32, y: f32, seed: u32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let (i, j) = (x0 as i32, y0 as i32);
        let top = hash(i, j, seed) * (1.0 - sx) + hash(i + 1, j, seed) * sx;
        let bottom = hash(i, j + 1, seed) * (1.0 - sx) + hash(i + 1, j + 1, seed) * sx;
        top * (1.0 - sy) + bottom * sy
    }

    /// A busy picture: value noise at several scales with hard-edged blobs on top.
    fn scene(w: u32, h: u32, seed: u32) -> RgbImage {
        let mut img = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let mut v = 0.0;
                for o in 0..5 {
                    v += noise(fx / (96.0 / (1 << o) as f32), fy / (96.0 / (1 << o) as f32), seed + o) * 0.5f32.powi(o as i32);
                }
                let g = (v * 150.0).clamp(0.0, 255.0);
                img.put_pixel(x, y, Rgb([g as u8, (g * 0.9 + 10.0) as u8, (g * 0.8 + 20.0) as u8]));
            }
        }
        let mut s = seed as u64 * 977 + 13;
        let mut rnd = |n: u32| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) as u32) % n
        };
        for _ in 0..260 {
            let (cx, cy, r) = (rnd(w) as i32, rnd(h) as i32, 4 + rnd(14) as i32);
            let c = Rgb([rnd(256) as u8, rnd(256) as u8, rnd(256) as u8]);
            for y in (cy - r).max(0)..(cy + r).min(h as i32) {
                for x in (cx - r).max(0)..(cx + r).min(w as i32) {
                    if (x - cx).abs() + (y - cy).abs() < r + r / 2 {
                        img.put_pixel(x as u32, y as u32, c);
                    }
                }
            }
        }
        img
    }

    fn bilinear(img: &RgbImage, x: f64, y: f64) -> Option<Rgb<u8>> {
        let (w, h) = img.dimensions();
        if x < 0.0 || y < 0.0 || x > w as f64 - 1.001 || y > h as f64 - 1.001 {
            return None;
        }
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (fx, fy) = (x - x0 as f64, y - y0 as f64);
        let px = |dx, dy| img.get_pixel(x0 + dx, y0 + dy);
        Some(Rgb(std::array::from_fn(|c| {
            let t = px(0, 0)[c] as f64 * (1.0 - fx) + px(1, 0)[c] as f64 * fx;
            let b = px(0, 1)[c] as f64 * (1.0 - fx) + px(1, 1)[c] as f64 * fx;
            (t * (1.0 - fy) + b * fy).round() as u8
        })))
    }

    const TILE: (u32, u32) = (900, 700);

    /// The map from scene to tile pixels for a view at `(ox, oy)`, slightly turned, zoomed and
    /// tilted.
    fn view(k: usize, ox: f64, oy: f64) -> H {
        let sign = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
        let (theta, s) = (0.02 * sign * (1.0 + k as f64 * 0.1), 1.0 + 0.01 * sign);
        let (c, sn) = (theta.cos() * s, theta.sin() * s);
        let a = [c, -sn, 0.0, sn, c, 0.0, 2e-5 * sign, -1.5e-5, 1.0];
        geom::mul(&a, &[1.0, 0.0, -ox, 0.0, 1.0, -oy, 0.0, 0.0, 1.0])
    }

    fn tile(scene: &RgbImage, p: &H) -> RgbImage {
        let inv = geom::inv(p).unwrap();
        let mut t = RgbImage::new(TILE.0, TILE.1);
        for y in 0..TILE.1 {
            for x in 0..TILE.0 {
                if let Some((sx, sy)) = geom::apply(&inv, x as f64, y as f64)
                    && let Some(c) = bilinear(scene, sx, sy)
                {
                    t.put_pixel(x, y, c);
                }
            }
        }
        t
    }

    fn tiles() -> (Vec<(String, RgbImage)>, Vec<H>) {
        let sc = scene(2100, 1300, 5);
        let origins = [(0.0, 0.0), (600.0, 20.0), (1200.0, 0.0), (30.0, 560.0), (620.0, 540.0), (1190.0, 570.0)];
        let views: Vec<H> = origins.iter().enumerate().map(|(k, o)| view(k, o.0, o.1)).collect();
        let imgs = views.iter().enumerate().map(|(k, p)| (format!("t{k}.png"), tile(&sc, p))).collect();
        (imgs, views)
    }

    #[test]
    fn tiles_with_rotation_and_perspective_are_registered_to_within_two_pixels() {
        let (imgs, views) = tiles();
        let reg = register(&imgs, &Progress::new()).unwrap();
        assert!(reg.h.iter().all(Option::is_some), "every tile is placed");
        // A scene point seen by two tiles must land on the same spot of the mosaic from both.
        let mut worst: f64 = 0.0;
        let mut checked = 0;
        for i in 0..imgs.len() {
            for j in i + 1..imgs.len() {
                for sx in (0..2100).step_by(60) {
                    for sy in (0..1300).step_by(60) {
                        let at = |k: usize| {
                            let (tx, ty) = geom::apply(&views[k], sx as f64, sy as f64)?;
                            if tx < 20.0 || ty < 20.0 || tx > TILE.0 as f64 - 20.0 || ty > TILE.1 as f64 - 20.0 {
                                return None;
                            }
                            geom::apply(&reg.h[k].unwrap(), tx, ty)
                        };
                        if let (Some(a), Some(b)) = (at(i), at(j)) {
                            worst = worst.max(((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt());
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 100, "{checked} shared points");
        assert!(worst < 2.0, "worst misalignment {worst:.2} px");
    }

    #[test]
    fn a_mosaic_is_the_size_of_what_the_tiles_cover() {
        let (mut imgs, _) = tiles();
        // Shuffled, and with a picture of something else that joins nothing.
        imgs.reverse();
        imgs.insert(2, ("stranger.png".into(), scene(TILE.0, TILE.1, 99)));
        let m = stitch(imgs, &Options { crop: false, ..Options::default() }, &Progress::new()).unwrap();
        assert_eq!(m.used, 6);
        assert_eq!(m.skipped, vec!["stranger.png".to_string()]);
        let (w, h) = m.image.dimensions();
        // The tiles span about 2100 × 1270 scene pixels.
        assert!((1900..2400).contains(&w) && (1150..1500).contains(&h), "{w}×{h}");
        let opaque = m.image.pixels().filter(|p| p[3] == 255).count() as f64 / (w * h) as f64;
        assert!(opaque > 0.8, "{opaque:.2} of the mosaic is covered");
    }

    #[test]
    fn photos_on_disk_are_stitched_as_jpeg_and_png() {
        let (imgs, _) = tiles();
        let dir = std::env::temp_dir().join(format!("layr-stitch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (k, (_, img)) in imgs.iter().enumerate() {
            img.save(dir.join(format!("tile{k}.{}", if k % 2 == 0 { "jpg" } else { "png" }))).unwrap();
        }
        let paths = job::images_in(&dir);
        assert_eq!(paths.len(), 6);
        let m = stitch_files(&paths, &Options::default(), &Progress::new());
        let _ = std::fs::remove_dir_all(&dir);
        let m = m.unwrap();
        assert_eq!(m.used, 6);
        assert!(m.skipped.is_empty());
    }

    #[test]
    fn a_cropped_mosaic_is_a_full_rectangle() {
        let (imgs, _) = tiles();
        let m = stitch(imgs, &Options::default(), &Progress::new()).unwrap();
        let (w, h) = m.image.dimensions();
        assert!(m.image.pixels().all(|p| p[3] == 255), "no hole is left");
        // The tiles turn and tilt a little, so the rectangle loses some of their 2100 × 1270.
        assert!(w > 1900 && h > 1100, "{w}×{h}");
    }

    #[test]
    fn the_largest_filled_rectangle_is_found() {
        // Opaque: a 6 × 3 block, and a taller but narrower 2 × 5 column.
        let mut img = RgbaImage::new(10, 8);
        let mut fill = |x0: u32, y0: u32, w: u32, h: u32| {
            for y in y0..y0 + h {
                for x in x0..x0 + w {
                    img.put_pixel(x, y, image::Rgba([1, 2, 3, 255]));
                }
            }
        };
        fill(1, 4, 6, 3);
        fill(8, 1, 2, 5);
        assert_eq!(largest_filled_rect(&img), Some((1, 4, 6, 3)));
        assert_eq!(largest_filled_rect(&RgbaImage::new(4, 4)), None);
        let full = RgbaImage::from_pixel(5, 3, image::Rgba([0, 0, 0, 255]));
        assert_eq!(largest_filled_rect(&full), Some((0, 0, 5, 3)));
    }

    #[test]
    fn the_mosaic_can_be_limited_in_size() {
        let (imgs, _) = tiles();
        let opts = Options { max_side: 800, ..Options::default() };
        let m = stitch(imgs, &opts, &Progress::new()).unwrap();
        assert!(m.image.width().max(m.image.height()) <= 800);
    }

    #[test]
    fn unrelated_images_are_refused() {
        let imgs = vec![("a".to_string(), scene(500, 400, 1)), ("b".to_string(), scene(500, 400, 2))];
        assert!(stitch(imgs, &Options::default(), &Progress::new()).is_err());
        assert!(stitch(vec![("a".to_string(), scene(100, 100, 1))], &Options::default(), &Progress::new()).is_err());
    }

    #[test]
    fn stitching_can_be_cancelled() {
        let (imgs, _) = tiles();
        let p = Progress::new();
        p.cancel();
        assert_eq!(stitch(imgs, &Options::default(), &p).err().as_deref(), Some("cancelled"));
    }
}

/// A developer check on real photos: `STITCH_DIR=<folder> STITCH_OUT=<file.png> cargo test
/// --release stitch::preview -- --ignored --nocapture`.
#[cfg(test)]
mod preview {
    #[test]
    #[ignore]
    fn preview() {
        let (Ok(dir), Ok(out)) = (std::env::var("STITCH_DIR"), std::env::var("STITCH_OUT")) else { return };
        let paths = super::job::images_in(std::path::Path::new(&dir));
        let t = std::time::Instant::now();
        let m = super::stitch_files(&paths, &super::Options::default(), &super::Progress::new()).unwrap();
        eprintln!(
            "{} images, {} used, skipped {:?}, {}×{} in {:?}",
            paths.len(),
            m.used,
            m.skipped,
            m.image.width(),
            m.image.height(),
            t.elapsed()
        );
        m.image.save(out).unwrap();
    }
}
