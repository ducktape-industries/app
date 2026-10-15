//! Decoding a view's raster into a gpui `RenderImage`, and the bound the
//! picture caches in pictures.rs share: the device's picture memory setting
//! (`backend::memory_bounds`), which also bounds what one decode allocates.
use super::*;
use image::ImageDecoder;

const ENTRY_BYTES: usize = std::mem::size_of::<(u64, (Arc<RenderImage>, u64))>();

fn picture_bytes() -> u64 {
    crate::backend::memory_bounds().picture_bytes
}

fn rgba_fits(width: u32, height: u32, budget: u64) -> bool {
    (u64::from(width) * u64::from(height)).saturating_mul(4) <= budget
}

pub(super) fn cache_fits(count: usize, used: usize, additional: usize, budget: u64) -> bool {
    let charged = (count.saturating_add(1))
        .saturating_mul(ENTRY_BYTES)
        .saturating_add(used)
        .saturating_add(additional);
    charged as u64 <= budget
}

pub(super) fn decode_image(data: &wire::ImageData) -> Option<RenderImage> {
    let mut pixels = match data {
        wire::ImageData::Resource(_) | wire::ImageData::Refusal(_) => return None,
        wire::ImageData::Rgba {
            width,
            height,
            pixels,
        } => image::RgbaImage::from_raw(*width, *height, pixels.clone())?,
        wire::ImageData::Encoded(bytes) => {
            let budget = picture_bytes();
            let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::no_limits();
            limits.max_alloc = Some(budget);
            reader.limits(limits);
            let decoder = reader.into_decoder().ok()?;
            let (width, height) = decoder.dimensions();
            // Decoder limits do not include expansion from grayscale to RGBA.
            if !rgba_fits(width, height, budget) {
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
        assert!(rgba_fits(4096, 4096, 64 << 20));
        assert!(!rgba_fits(4097, 4096, 64 << 20));
        assert!(!rgba_fits(u32::MAX, u32::MAX, 64 << 20));
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
    fn a_header_claiming_more_than_the_picture_bytes_is_refused_unallocated() {
        let side = 300_000u32;
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], data: &[u8]| {
            png.extend((data.len() as u32).to_be_bytes());
            let body = [kind, data].concat();
            png.extend(&body);
            let mut crc = 0xffff_ffffu32;
            for byte in &body {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = match crc & 1 {
                        1 => (crc >> 1) ^ 0xedb8_8320,
                        _ => crc >> 1,
                    };
                }
            }
            png.extend((!crc).to_be_bytes());
        };
        let header = [
            &side.to_be_bytes()[..],
            &side.to_be_bytes(),
            &[8, 6, 0, 0, 0],
        ]
        .concat();
        chunk(b"IHDR", &header);
        chunk(b"IDAT", &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
        chunk(b"IEND", &[]);
        assert!(decode_image(&wire::ImageData::Encoded(png)).is_none());
    }

    #[test]
    fn the_cache_charges_each_raster_its_bytes_and_its_entry() {
        let budget = 10 * ENTRY_BYTES as u64 + 100;
        assert!(cache_fits(9, 100, 0, budget));
        assert!(!cache_fits(10, 100, 0, budget));
        assert!(!cache_fits(9, 100, 1, budget));
        assert!(!cache_fits(1, 0, usize::MAX, budget));
    }
}
