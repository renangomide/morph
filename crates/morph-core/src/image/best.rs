// SPDX-License-Identifier: GPL-3.0-or-later
//! Compress "Best (automatic)" (SPEC §4.1): the smallest file that still looks
//! identical to the original, judged by the SSIMULACRA2 perceptual metric
//! (`fast-ssim2`).
//!
//! - [`Reference`]: the original prepared once for scoring candidates. Large
//!   images are judged on a [sample](Reference::sample): a mosaic of full-size
//!   tiles, which is also what the quality search encodes (much faster than
//!   encoding the whole photo each time).
//! - [`search_lowest_passing`]: binary search for the lowest quality that passes.
//!
//! Limitation: pixels are scored as 8-bit sRGB. 16-bit images are reduced to
//! 8 bits first, and wide-gamut images (e.g. a Display P3 ICC profile) are
//! scored as if they were sRGB; both images get the same treatment, so the
//! comparison stays fair, only slightly less precise.

use crate::error::ConvertError;
use crate::image::io::flatten;
use fast_ssim2::{PixelDescriptor, PixelSlice, Ssimulacra2Reference};
use image::{DynamicImage, GenericImageView};

/// SSIMULACRA2 score a candidate must reach to count as "looks identical".
///
/// The libjxl scale, for images viewed at 1:1 (which is how they are scored
/// here): 70 = high quality (artifacts hard to notice without the original),
/// 80 = very high (can't be told apart side by side), 85 = excellent (can't be
/// told apart even when flipping between them), 90 = visually lossless (flicker
/// test). 80 is "looks the same" for a normal look at the photo; the margin of 2
/// covers that large images are judged on a sample and that scores don't
/// always rise smoothly with quality.
pub const LOOKS_IDENTICAL_SCORE: f64 = 82.0;

/// Qualities searched for JPG/WebP/AVIF. Below 60 photos practically never pass
/// the threshold; above 95 files grow a lot for no visible gain.
pub const BEST_QUALITY_RANGE: (u8, u8) = (60, 95);

/// First quality tried: where most photos land, so the search narrows fast.
pub const FIRST_GUESS: u8 = 82;

/// Trial encodes per quality search: the first guess, then a binary search.
/// Over 36 qualities this lands within 1–2 of the exact lowest passing quality,
/// which isn't worth two more encodes.
pub const MAX_BEST_TRIES: u32 = 5;

/// After a search on the sample, the full image is encoded and checked once;
/// if it fails, the quality is raised by this much and checked again, at most
/// [`MAX_CONFIRM_RETRIES`] times.
pub const CONFIRM_STEP: u8 = 4;
pub const MAX_CONFIRM_RETRIES: u32 = 2;

/// Images above this many pixels are judged on a sample of about this size:
/// a mosaic of [`TILE`]-pixel tiles at 1:1 (artifacts keep their full strength,
/// unlike in a downscaled copy). Half the tiles are spread evenly over the
/// image; a quarter are its busiest areas (where ringing shows) and a quarter
/// its flattest non-uniform areas (where blocking and banding show).
/// On test photos a 1 MP sample picked the same quality as 2 and 4 MP ones, and
/// keeps a trial (encode, decode, score) around 0.2 s.
pub const METRIC_MAX_PIXELS: u64 = 1_000_000;

/// Side of the sample tiles, in pixels: a multiple of 16, so tiles line up
/// with JPEG blocks (8×8, 16×16 with chroma subsampling) and WebP macroblocks.
const TILE: u32 = 128;

/// Mean luma step (0–255) below which a tile counts as uniform (e.g. a black
/// border), not as a "flat" area worth checking for banding.
const UNIFORM_ACTIVITY: f32 = 0.1;

/// Peak memory of the metric per pixel scored (measured ~135 bytes).
const METRIC_BYTES_PER_PIXEL: u64 = 160;

/// Extra memory Best mode needs for an image of `dims` (`None` = unknown size:
/// assume the largest sample).
pub fn metric_memory(dims: Option<(u32, u32)>) -> u64 {
    let pixels = dims.map_or(METRIC_MAX_PIXELS, |(w, h)| u64::from(w) * u64::from(h));
    pixels.min(METRIC_MAX_PIXELS) * METRIC_BYTES_PER_PIXEL
}

fn metric_failed(details: impl ToString) -> ConvertError {
    ConvertError::other("Couldn't compare the compressed image.", details)
}

/// Where the sample tiles of a large image come from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Layout {
    /// Tile side.
    tile: u32,
    /// Top-left corners of the chosen tiles in the image, in mosaic order.
    tiles: Vec<(u32, u32)>,
    /// Tiles per mosaic row.
    cols: u32,
}

impl Layout {
    fn mosaic_size(&self) -> (u32, u32) {
        let rows = self.tiles.len() as u32 / self.cols.max(1);
        (self.cols * self.tile, rows * self.tile)
    }
}

/// Mean absolute luma step inside a tile, on a sparse grid (every 4th pixel).
fn activity(img: &DynamicImage, x0: u32, y0: u32, tile: u32) -> f32 {
    let luma = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        (u32::from(p[0]) * 2 + u32::from(p[1]) * 5 + u32::from(p[2])) as f32 / 8.0
    };
    let step = 4.min(tile.saturating_sub(1)).max(1);
    let (mut sum, mut n) = (0.0f32, 0u32);
    let mut y = y0;
    while y + step < y0 + tile {
        let mut x = x0;
        while x + step < x0 + tile {
            let c = luma(x, y);
            sum += (luma(x + step, y) - c).abs() + (luma(x, y + step) - c).abs();
            n += 2;
            x += step;
        }
        y += step;
    }
    if n == 0 { 0.0 } else { sum / n as f32 }
}

/// Sample layout for `img`, or `None` to score the whole image.
fn layout(img: &DynamicImage, max_pixels: u64) -> Option<Layout> {
    let (w, h) = (img.width(), img.height());
    if u64::from(w) * u64::from(h) <= max_pixels {
        return None;
    }
    // Very thin images: smaller tiles, still on the 16-pixel grid when possible.
    let mut tile = TILE.min(w).min(h);
    if tile >= 16 {
        tile -= tile % 16;
    }
    let (gw, gh) = (w / tile, h / tile);
    let cells: Vec<(u32, u32)> = (0..gh)
        .flat_map(|j| (0..gw).map(move |i| (i * tile, j * tile)))
        .collect();
    let budget = (max_pixels / (u64::from(tile) * u64::from(tile))).max(1) as usize;
    let want = budget.min(cells.len()).max(1);
    let cols = ((want as f64).sqrt().floor() as usize).max(1);
    let n = cols * (want / cols);

    // Rank by activity; take the busiest and the flattest non-uniform quarters,
    // then fill up with tiles spread evenly over the image.
    let act: Vec<f32> = cells
        .iter()
        .map(|&(x, y)| activity(img, x, y, tile))
        .collect();
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by(|&a, &b| act[a].total_cmp(&act[b]));
    let mut chosen = vec![false; cells.len()];
    let mut count = 0;
    let mut take = |i: usize, chosen: &mut Vec<bool>| {
        if count < n && !chosen[i] {
            chosen[i] = true;
            count += 1;
        }
    };
    for &i in order.iter().rev().take(n / 4) {
        take(i, &mut chosen);
    }
    for &i in order
        .iter()
        .filter(|&&i| act[i] >= UNIFORM_ACTIVITY)
        .take(n / 4)
    {
        take(i, &mut chosen);
    }
    let left: Vec<usize> = (0..cells.len()).filter(|&i| !chosen[i]).collect();
    let need = n.saturating_sub(chosen.iter().filter(|&&c| c).count());
    if need > 0 && !left.is_empty() {
        for k in 0..need {
            let i = left[(k * left.len() / need).min(left.len() - 1)];
            take(i, &mut chosen);
        }
    }
    let tiles: Vec<(u32, u32)> = (0..cells.len())
        .filter(|&i| chosen[i])
        .map(|i| cells[i])
        .collect();
    // `tiles.len() == n`, a multiple of `cols`, unless the image had too few cells.
    let cols = cols.min(tiles.len()).max(1);
    let tiles = tiles[..tiles.len() - tiles.len() % cols].to_vec();
    Some(Layout {
        tile,
        tiles,
        cols: cols as u32,
    })
}

/// The tiles of `layout`, cut straight from `img` and put side by side.
fn mosaic(img: &DynamicImage, layout: &Layout) -> DynamicImage {
    let (mw, mh) = layout.mosaic_size();
    let mut out = DynamicImage::new(mw, mh, img.color());
    for (k, &(x, y)) in layout.tiles.iter().enumerate() {
        let k = k as u32;
        let (dx, dy) = (
            (k % layout.cols) * layout.tile,
            (k / layout.cols) * layout.tile,
        );
        let tile = img.view(x, y, layout.tile, layout.tile);
        image::imageops::replace(&mut out, &*tile, i64::from(dx), i64::from(dy));
    }
    out
}

/// 8-bit sRGB pixels in a layout the metric reads.
struct Pixels {
    data: Vec<u8>,
    width: u32,
    height: u32,
    alpha: bool,
}

impl Pixels {
    /// `img` as RGB8, or RGBA8 if it has transparency and `background` is `None`.
    fn of(img: &DynamicImage, background: Option<[u8; 3]>) -> Pixels {
        let (width, height) = (img.width(), img.height());
        let (data, alpha) = match background {
            None if img.color().has_alpha() => (img.to_rgba8().into_raw(), true),
            None => (img.to_rgb8().into_raw(), false),
            Some(bg) => (flatten(img, bg).into_raw(), false),
        };
        Pixels {
            data,
            width,
            height,
            alpha,
        }
    }

    fn slice(&self) -> Result<PixelSlice<'_>, ConvertError> {
        let (bpp, descriptor) = if self.alpha {
            (4, PixelDescriptor::RGBA8_SRGB)
        } else {
            (3, PixelDescriptor::RGB8_SRGB)
        };
        PixelSlice::new(
            &self.data,
            self.width,
            self.height,
            self.width as usize * bpp,
            descriptor,
        )
        .map_err(|e| metric_failed(format!("{e:?}")))
    }
}

/// The original image, ready to score candidates against.
pub struct Reference {
    /// Original size.
    dims: (u32, u32),
    /// Sample tiles for large images.
    layout: Option<Layout>,
    /// Flatten candidates on this background (the output has no transparency).
    background: Option<[u8; 3]>,
    reference: Ssimulacra2Reference,
}

impl Reference {
    /// Prepare `img` (upright, full size). `keep_alpha`: the output format keeps
    /// transparency (the metric then judges it on a dark and a light background);
    /// otherwise transparent pixels are judged as the encoder flattens them, on
    /// `background`.
    pub fn new(
        img: &DynamicImage,
        keep_alpha: bool,
        background: [u8; 3],
    ) -> Result<Reference, ConvertError> {
        let layout = layout(img, METRIC_MAX_PIXELS);
        let background = (img.color().has_alpha() && !keep_alpha).then_some(background);
        let sample = layout.as_ref().map(|l| mosaic(img, l));
        let pixels = Pixels::of(sample.as_ref().unwrap_or(img), background);
        drop(sample);
        let reference = Ssimulacra2Reference::new(&pixels.slice()?).map_err(metric_failed)?;
        Ok(Reference {
            dims: (img.width(), img.height()),
            layout,
            background,
            reference,
        })
    }

    /// The part of a full-size image that is scored: `None` = the whole image
    /// (small images), else the sample mosaic. Encoding the sample instead of
    /// the whole image gives (nearly) the same score, much faster.
    pub fn sample(&self, img: &DynamicImage) -> Option<DynamicImage> {
        self.layout.as_ref().map(|l| mosaic(img, l))
    }

    /// SSIMULACRA2 score of a full-size `candidate` (decoded). 100 = identical.
    pub fn score(&self, candidate: &DynamicImage) -> Result<f64, ConvertError> {
        let dims = (candidate.width(), candidate.height());
        if dims != self.dims {
            return Err(metric_failed(format!(
                "size mismatch: {dims:?} vs {:?}",
                self.dims
            )));
        }
        match self.sample(candidate) {
            Some(sample) => self.score_pixels(&sample),
            None => self.score_pixels(candidate),
        }
    }

    /// Score of an encoded-and-decoded [`Reference::sample`] (the whole image if
    /// there is no sample).
    pub fn score_sample(&self, sample: &DynamicImage) -> Result<f64, ConvertError> {
        self.score_pixels(sample)
    }

    fn score_pixels(&self, img: &DynamicImage) -> Result<f64, ConvertError> {
        self.reference
            .compare(&Pixels::of(img, self.background).slice()?)
            .map_err(metric_failed)
    }
}

/// Whether a score counts as "looks identical".
pub fn looks_identical(score: f64) -> bool {
    score >= LOOKS_IDENTICAL_SCORE
}

/// One step of [`search_lowest_passing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trial {
    /// Looks identical (bytes kept as the result so far).
    Pass(Vec<u8>),
    /// Visibly different: higher quality needed.
    Fail,
    /// Not smaller than the input, so not judged: lower quality needed.
    TooBig,
}

/// Search `lo..=hi` (at most `max_tries` calls, starting at `first` if inside
/// the range) for the lowest quality whose encoding passes. An error (e.g.
/// cancel) stops the search. Assumes higher quality never looks worse and is
/// never smaller. Returns the lowest passing quality and its bytes.
pub fn search_lowest_passing<F>(
    (mut lo, mut hi): (u8, u8),
    first: Option<u8>,
    max_tries: u32,
    mut trial: F,
) -> Result<Option<(u8, Vec<u8>)>, ConvertError>
where
    F: FnMut(u8) -> Result<Trial, ConvertError>,
{
    let mut best = None;
    let mut tries = 0;
    let mut next = first.filter(|q| (lo..=hi).contains(q));
    while lo <= hi && tries < max_tries {
        let q = next.take().unwrap_or(lo + (hi - lo) / 2);
        tries += 1;
        let lower = match trial(q)? {
            Trial::Pass(bytes) => {
                best = Some((q, bytes));
                true
            }
            Trial::TooBig => true,
            Trial::Fail => false,
        };
        if lower {
            match q.checked_sub(1) {
                Some(prev) => hi = prev,
                None => break,
            }
        } else {
            match q.checked_add(1) {
                Some(n) => lo = n,
                None => break,
            }
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn fake_search(threshold: u8, too_big_from: u8) -> (Option<(u8, Vec<u8>)>, u32) {
        let mut calls = 0;
        let r = search_lowest_passing(BEST_QUALITY_RANGE, Some(FIRST_GUESS), MAX_BEST_TRIES, |q| {
            calls += 1;
            Ok(if q >= too_big_from {
                Trial::TooBig
            } else if q >= threshold {
                Trial::Pass(vec![q])
            } else {
                Trial::Fail
            })
        })
        .unwrap();
        (r, calls)
    }

    #[test]
    fn search_finds_lowest_passing_quality() {
        for threshold in [60u8, 61, 70, 81, 82, 83, 90, 94, 95] {
            let (r, calls) = fake_search(threshold, 255);
            let (q, bytes) = r.unwrap();
            assert!((threshold..=threshold + 2).contains(&q), "{threshold}: {q}");
            assert_eq!(bytes, vec![q]);
            assert!(calls <= MAX_BEST_TRIES, "{threshold}: {calls}");
        }
    }

    #[test]
    fn search_skips_results_bigger_than_the_input() {
        // Passes from 70, but 80 and up is bigger than the input.
        let q = fake_search(70, 80).0.unwrap().0;
        assert!((70..=72).contains(&q), "{q}");
        // Everything that passes is too big: nothing.
        assert_eq!(fake_search(85, 80).0, None);
    }

    #[test]
    fn search_reports_nothing_when_nothing_passes() {
        let r = search_lowest_passing(BEST_QUALITY_RANGE, None, MAX_BEST_TRIES, |_| {
            Ok(Trial::Fail)
        })
        .unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn search_stops_on_error() {
        let mut calls = 0;
        let r = search_lowest_passing(BEST_QUALITY_RANGE, None, MAX_BEST_TRIES, |q| {
            calls += 1;
            if calls == 3 {
                Err(ConvertError::cancelled())
            } else {
                Ok(Trial::Pass(vec![q]))
            }
        });
        assert!(r.is_err_and(|e| e.is_cancelled()));
        assert_eq!(calls, 3, "no encode after the error");
    }

    /// Smooth left half, busy right half, uniform black bottom strip.
    fn mixed(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            if y > h * 3 / 4 {
                Rgb([0, 0, 0])
            } else if x < w / 2 {
                Rgb([(x / 4) as u8, (y / 4) as u8, 100])
            } else {
                let v = ((x * 7919 + y * 104_729) % 251) as u8;
                Rgb([v, v / 2, 255 - v])
            }
        }))
    }

    #[test]
    fn small_images_are_scored_whole() {
        assert_eq!(layout(&mixed(1000, 1000), METRIC_MAX_PIXELS), None);
    }

    #[test]
    fn large_images_are_sampled_in_aligned_tiles() {
        for (w, h) in [
            (4000, 3000),
            (3000, 4000),
            (6000, 4200),
            (40000, 300),
            (300, 40000),
        ] {
            let img = mixed(w, h);
            let l = layout(&img, METRIC_MAX_PIXELS).unwrap();
            let t = l.tile;
            assert_eq!(t % 16, 0, "{w}×{h}");
            let (mw, mh) = l.mosaic_size();
            let total = u64::from(mw) * u64::from(mh);
            assert!(total <= METRIC_MAX_PIXELS, "{w}×{h}: {total}");
            assert!(total >= METRIC_MAX_PIXELS * 3 / 4, "{w}×{h}: {total}");
            assert_eq!(l.tiles.len() as u32 % l.cols, 0);
            let mut seen = std::collections::HashSet::new();
            for &(x, y) in &l.tiles {
                assert!(x % 16 == 0 && y % 16 == 0 && x + t <= w && y + t <= h);
                assert!(seen.insert((x, y)), "tile chosen twice");
            }
        }
    }

    #[test]
    fn sample_covers_busy_flat_and_spread_tiles() {
        let img = mixed(4096, 3072);
        let l = layout(&img, METRIC_MAX_PIXELS).unwrap();
        let busy = l
            .tiles
            .iter()
            .filter(|&&(x, y)| x >= 2048 && y < 2304)
            .count();
        let flat = l
            .tiles
            .iter()
            .filter(|&&(x, y)| x < 2048 && y < 2304)
            .count();
        // The busiest quarter all comes from the busy half; spread tiles add more.
        assert!(busy >= l.tiles.len() / 4, "{busy} of {}", l.tiles.len());
        assert!(flat >= l.tiles.len() / 4, "{flat} of {}", l.tiles.len());
    }

    #[test]
    fn mosaic_copies_tiles_at_full_resolution() {
        let img =
            DynamicImage::ImageRgb8(RgbImage::from_fn(10, 6, |x, y| Rgb([x as u8, y as u8, 0])));
        let l = Layout {
            tile: 2,
            tiles: vec![(0, 3), (8, 3)],
            cols: 2,
        };
        let m = mosaic(&img, &l).to_rgb8();
        assert_eq!(m.dimensions(), (4, 2));
        assert_eq!(m.get_pixel(0, 0).0[..2], [0, 3]);
        assert_eq!(m.get_pixel(2, 0).0[..2], [8, 3]);
        assert_eq!(m.get_pixel(3, 1).0[..2], [9, 4]);
    }

    #[test]
    fn identical_scores_high_and_alpha_errors_count() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_fn(64, 64, |x, y| {
            Rgba([
                (x * 4) as u8,
                (y * 4) as u8,
                90,
                if x < 32 { 0 } else { 255 },
            ])
        }));
        let reference = Reference::new(&img, true, [255; 3]).unwrap();
        assert!(reference.score(&img).unwrap() > 99.0);
        // Making the transparent half opaque is a big visible change.
        let mut opaque = img.to_rgba8();
        for p in opaque.pixels_mut() {
            p[3] = 255;
        }
        let opaque = DynamicImage::ImageRgba8(opaque);
        assert!(!looks_identical(reference.score(&opaque).unwrap()));
        // Tiny images are padded by the metric, not rejected.
        let tiny = DynamicImage::new_rgb8(3, 5);
        let reference = Reference::new(&tiny, false, [255; 3]).unwrap();
        assert!(looks_identical(reference.score(&tiny).unwrap()));
    }

    #[test]
    fn sampled_score_matches_whole_image_score() {
        let img = mixed(2048, 1536);
        let reference = Reference::new(&img, false, [255; 3]).unwrap();
        let sample = reference.sample(&img).unwrap();
        assert!(reference.score_sample(&sample).unwrap() > 99.0);
        assert!(reference.score(&img).unwrap() > 99.0);
    }
}
