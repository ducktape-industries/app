use super::*;
use image::ImageDecoder;

const MAX_PICTURE_BYTES: usize = 64 << 20;
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
            decode_image(&wire::ImageData::Rgba {
                width: u32::MAX,
                height: u32::MAX,
                pixels: Vec::new(),
            })
            .is_none()
        );
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
