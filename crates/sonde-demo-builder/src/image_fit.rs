//! Resize an image and JPEG-encode it under a target byte budget by
//! searching downward on quality.

use anyhow::{bail, Result};
use image::{codecs::jpeg::JpegEncoder, DynamicImage};

/// Resize `img` so its longest side is at most `max_dim`, then JPEG-encode,
/// lowering quality until the encoded size is <= `target_bytes`. Returns the
/// encoded JPEG bytes. Errors only if even quality 10 overflows the budget.
#[allow(dead_code)] // called in Task 4; removed then
pub fn fit_jpeg(img: &DynamicImage, max_dim: u32, target_bytes: usize) -> Result<Vec<u8>> {
    let resized = img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle);
    let rgb = resized.to_rgb8();
    for quality in (10..=90).rev().step_by(5) {
        let mut buf = Vec::new();
        {
            let mut enc = JpegEncoder::new_with_quality(&mut buf, quality as u8);
            enc.encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )?;
        }
        if buf.len() <= target_bytes {
            return Ok(buf);
        }
    }
    bail!("could not fit image under {target_bytes} bytes even at quality 10");
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn noise_image(w: u32, h: u32) -> DynamicImage {
        // Deterministic pseudo-random pixels (JPEG-incompressible-ish) so the
        // quality search has to actually lower quality to hit a small budget.
        let mut state: u32 = 0x1234_5678;
        let mut img = RgbImage::new(w, h);
        for p in img.pixels_mut() {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let v = (state >> 16) as u8;
            *p = image::Rgb([v, v.wrapping_add(40), v.wrapping_add(80)]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn fits_under_budget() {
        let img = noise_image(640, 480);
        let bytes = fit_jpeg(&img, 200, 5000).expect("should fit");
        assert!(bytes.len() <= 5000, "got {} bytes", bytes.len());
        // Valid JPEG SOI marker.
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8]);
    }
}
