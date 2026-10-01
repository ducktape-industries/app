//! Decoding a guest's raster into a gpui `RenderImage`, and the caps the
//! picture caches in pictures.rs share: one picture's bytes, and a whole
//! cache's bytes and entries.
use super::*;
use image::ImageDecoder;

/// Both the one-picture ceiling (decoded size, decoder allocation) and a
/// whole cache's byte ceiling.
const MAX_PICTURE_BYTES: usize = 64 << 20;
/// A whole cache's entry ceiling.
const MAX_PICTURES: usize = 4096;

fn rgba_fits(width: u32, height: u32) -> bool {
    width <= 8192
        && height <= 8192
        && u64::from(width) * u64::from(height) * 4 <= MAX_PICTURE_BYTES as u64
}

pub(super) fn cache_fits(count: usize, used: usize, additional: usize) -> bool {
    count < MAX_PICTURES && additional <= MAX_PICTURE_BYTES.saturating_sub(used)
}

pub(super) fn decode_image(data: &wire::ImageData) -> Option<RenderImage> {
    let mut pixels = match data {
        wire::ImageData::Resource(_) | wire::ImageData::Refusal(_) => return None,
        wire::ImageData::Rgba {
            width,
            height,
            pixels,
        } => {
            if !rgba_fits(*width, *height) {
                return None;
            }
            image::RgbaImage::from_raw(*width, *height, pixels.clone())?
        }
        wire::ImageData::Encoded(bytes) => {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(MAX_PICTURE_BYTES as u64);
            reader.limits(limits);
            let decoder = reader.into_decoder().ok()?;
            let (width, height) = decoder.dimensions();
            // Decoder limits do not include expansion from grayscale to RGBA.
            if !rgba_fits(width, height) {
                return None;
            }
            image::DynamicImage::from_decoder(decoder)
                .ok()?
                .into_rgba8()
        }
    };
    // gpui RenderImage frames are BGRA.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(RenderImage::new(vec![image::Frame::new(pixels)]))
}

#[cfg(test)]
mod resource_limits {
    use super::*;

    #[test]
    fn rgba_expansion_is_bounded_before_allocation() {
        assert!(rgba_fits(4096, 4096));
        assert!(!rgba_fits(4097, 4096));
        assert!(!rgba_fits(8192, 8192));
        assert!(!rgba_fits(u32::MAX, u32::MAX));
        assert!(
            // sized right, so only `rgba_fits` can refuse it
            decode_image(&wire::ImageData::Rgba {
                width: 8193,
                height: 1,
                pixels: vec![0; 8193 * 4],
            })
            .is_none()
        );
        // the decoder's own limit passes an 8-bit grayscale picture its RGBA
        // expansion would not fit
        let mut png = Vec::new();
        image::GrayImage::new(4097, 4096)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(decode_image(&wire::ImageData::Encoded(png)).is_none());
    }

    #[test]
    fn cache_limits_bound_both_bytes_and_empty_entries() {
        assert!(cache_fits(MAX_PICTURES - 1, MAX_PICTURE_BYTES - 4, 4));
        assert!(!cache_fits(MAX_PICTURES, 0, 0));
        assert!(!cache_fits(1, MAX_PICTURE_BYTES - 4, 5));
        assert!(!cache_fits(1, MAX_PICTURE_BYTES, 1));
        assert!(!cache_fits(1, 0, usize::MAX));
    }
}
