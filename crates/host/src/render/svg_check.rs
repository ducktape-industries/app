//! A view's SVG reaches gpui's parser only when every image it names is a data-URL raster within
//! the rasterizer's caps; a file, a nested SVG or gzip refuses the whole document.
use image::{ImageFormat, ImageReader};
use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_RASTER_SIDE: u32 = 8192;
const MAX_RASTER_PIXELS: u64 = 1 << 24;

pub(crate) fn parse(bytes: &[u8]) -> Result<usvg::Tree, &'static str> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err("Compressed SVG data refused");
    }
    let named = Arc::new(AtomicBool::new(false));
    let (by_data, by_string) = (named.clone(), named.clone());
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(move |mime, data, _| {
                let kind = raster(mime, data);
                by_data.fetch_or(kind.is_none(), Ordering::Relaxed);
                kind
            }),
            resolve_string: Box::new(move |_, _| {
                by_string.store(true, Ordering::Relaxed);
                None
            }),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|_| "SVG data unreadable")?;
    if named.load(Ordering::Relaxed) {
        return Err("SVG naming a file or another SVG refused");
    }
    Ok(tree)
}

fn raster_fits(width: u32, height: u32) -> bool {
    width <= MAX_RASTER_SIDE
        && height <= MAX_RASTER_SIDE
        && u64::from(width) * u64::from(height) <= MAX_RASTER_PIXELS
}

fn webp_frames_fit(data: &[u8]) -> bool {
    data.get(8..12) == Some(&b"WEBP"[..])
        && data
            .get(12..)
            .is_some_and(|chunks| webp_chunks_fit(chunks, true))
}

fn webp_chunks_fit(mut chunks: &[u8], top_level: bool) -> bool {
    let side = |low: u8, high: u8| u32::from(u16::from_le_bytes([low, high]) & 0x3FFF);
    while let Some(header) = chunks.get(..8) {
        let Ok(size) = <[u8; 4]>::try_from(&header[4..]) else {
            return false;
        };
        let size = u32::from_le_bytes(size) as usize;
        let rest = &chunks[8..];
        let Some(body) = rest.get(..size) else {
            return false;
        };
        let fits = match &header[..4] {
            b"VP8 " => body
                .get(6..10)
                .is_some_and(|d| raster_fits(side(d[0], d[1]), side(d[2], d[3]))),
            b"VP8L" => body.get(1..5).is_some_and(|d| {
                let bits = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
                raster_fits((bits & 0x3FFF) + 1, (bits >> 14 & 0x3FFF) + 1)
            }),
            b"ANMF" => top_level && body.get(16..).is_some_and(|f| webp_chunks_fit(f, false)),
            _ => true,
        };
        if !fits {
            return false;
        }
        chunks = rest.get(size + size % 2..).unwrap_or_default();
    }
    true
}

fn gif_first_frame_fits(data: &[u8]) -> bool {
    let Ok(mut decoder) = gif::DecodeOptions::new().read_info(data) else {
        return false;
    };
    matches!(decoder.next_frame_info(), Ok(Some(frame))
        if raster_fits(frame.width.into(), frame.height.into()))
}

fn raster(mime: &str, data: Arc<Vec<u8>>) -> Option<usvg::ImageKind> {
    let format = match mime {
        "image/jpg" | "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        "text/plain" => image::guess_format(&data).ok()?,
        _ => return None,
    };
    let kind: fn(Arc<Vec<u8>>) -> usvg::ImageKind = match format {
        ImageFormat::Jpeg => usvg::ImageKind::JPEG,
        ImageFormat::Png => usvg::ImageKind::PNG,
        ImageFormat::Gif => usvg::ImageKind::GIF,
        ImageFormat::WebP => usvg::ImageKind::WEBP,
        _ => return None,
    };
    let mut reader = ImageReader::with_format(Cursor::new(data.as_slice()), format);
    reader.no_limits();
    let (width, height) = reader.into_dimensions().ok()?;
    let frames_fit = match format {
        ImageFormat::WebP => webp_frames_fit(&data),
        ImageFormat::Gif => gif_first_frame_fits(&data),
        _ => true,
    };
    (raster_fits(width, height) && frames_fit).then(|| kind(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUB_SVG: &str = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0IiBoZWlnaHQ9IjQiPjxyZWN0IHdpZHRoPSI0IiBoZWlnaHQ9IjQiLz48L3N2Zz4=";
    const SUB_SVGZ: &str = "H4sIAAAAAAACA7MpLktXqMjNySu2VcooKSmw0tcvLy/XKzfWyy9K1zcyMDDQB6pQUijPTCnJsFUyUVLISM1MzygBMe1silKTS7BK6dvZgPTZAQAT6jFqXwAAAA==";
    const RED_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";

    fn find_image(group: &usvg::Group) -> Option<&usvg::Image> {
        group.children().iter().find_map(|node| match node {
            usvg::Node::Image(image) => Some(&**image),
            usvg::Node::Group(group) => find_image(group),
            _ => None,
        })
    }

    fn svg_with_image(href: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><image href="{href}" width="4" height="4"/></svg>"#
        )
    }

    fn shown(href: &str) -> bool {
        parse(svg_with_image(href).as_bytes()).is_ok()
    }

    fn data_url(mime: &str, bytes: &[u8]) -> String {
        let body: String = bytes.iter().map(|byte| format!("%{byte:02X}")).collect();
        format!("data:{mime},{body}")
    }

    #[test]
    fn a_view_svg_naming_a_local_file_is_refused_whole() {
        let path = std::env::temp_dir().join(format!("host-svg-href-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let parsed = parse(svg_with_image(&path.to_string_lossy()).as_bytes());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            parsed.err(),
            Some("SVG naming a file or another SVG refused")
        );
    }

    #[test]
    fn a_nested_svg_data_href_is_refused() {
        let shown: Vec<String> = [
            format!("data:image/svg+xml;base64,{SUB_SVG}"),
            format!("data:text/plain;base64,{SUB_SVG}"),
            format!("data:image/svg+xml;base64,{SUB_SVGZ}"),
        ]
        .into_iter()
        .filter(|href| shown(href))
        .collect();
        assert_eq!(shown, Vec::<String>::new());
    }

    #[test]
    fn a_raster_data_href_still_parses_as_an_image() {
        for href in [
            format!("data:image/png;base64,{RED_PNG}"),
            format!("data:;base64,{RED_PNG}"),
        ] {
            let tree = parse(svg_with_image(&href).as_bytes()).unwrap();
            let image = find_image(tree.root());
            assert!(
                matches!(image.map(|i| i.kind()), Some(usvg::ImageKind::PNG(_))),
                "{href}"
            );
        }
    }

    #[test]
    fn compressed_svg_data_is_refused_before_parsing() {
        assert_eq!(
            parse(&[0x1f, 0x8b, 0x08, 0x00]).err(),
            Some("Compressed SVG data refused")
        );
        assert!(parse(b"<svg xmlns='http://www.w3.org/2000/svg'/>").is_ok());
    }

    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let chunk = |kind: &[u8], body: &[u8]| {
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(body);
            let len = (body.len() as u32).to_be_bytes();
            [&len[..], kind, body, &crc.sum().to_be_bytes()].concat()
        };
        let ihdr = [
            &width.to_be_bytes()[..],
            &height.to_be_bytes(),
            &[8, 6, 0, 0, 0],
        ]
        .concat();
        [
            &b"\x89PNG\r\n\x1a\n"[..],
            &chunk(b"IHDR", &ihdr),
            &chunk(b"IDAT", &[]),
            &chunk(b"IEND", &[]),
        ]
        .concat()
    }

    #[test]
    fn a_raster_data_href_past_the_size_caps_is_refused() {
        let png = |(width, height)| data_url("image/png", &png_header(width, height));
        assert!(shown(&png((MAX_RASTER_SIDE, 2048))));
        let over: Vec<_> = [(16383, 16383), (MAX_RASTER_SIDE + 1, 1), (4097, 4097)]
            .into_iter()
            .filter(|size| shown(&png(*size)))
            .collect();
        assert_eq!(over, Vec::<(u32, u32)>::new());
    }

    fn riff_chunk(fourcc: &[u8], body: &[u8]) -> Vec<u8> {
        let pad: &[u8] = if body.len() % 2 == 1 { &[0] } else { &[] };
        [fourcc, &(body.len() as u32).to_le_bytes(), body, pad].concat()
    }

    fn vp8(width: u16, height: u16) -> Vec<u8> {
        let header = [0, 0, 0, 0x9d, 0x01, 0x2a];
        let body = [&header[..], &width.to_le_bytes(), &height.to_le_bytes()].concat();
        riff_chunk(b"VP8 ", &body)
    }

    fn vp8l(width: u32, height: u32) -> Vec<u8> {
        let bits = (width - 1) | (height - 1) << 14;
        riff_chunk(b"VP8L", &[&[0x2f][..], &bits.to_le_bytes()].concat())
    }

    fn webp_file(canvas: u32, animated: bool, frame: Vec<u8>, riff_covers_frame: bool) -> String {
        let less_one = (canvas - 1).to_le_bytes();
        let flags = if animated { 0x02 } else { 0 };
        let vp8x = [&[flags, 0, 0, 0][..], &less_one[..3], &less_one[..3]].concat();
        let mut chunks = riff_chunk(b"VP8X", &vp8x);
        if animated {
            chunks.extend(riff_chunk(b"ANIM", &[0; 6]));
        }
        let before_frame = chunks.len();
        if animated {
            let anmf = [&[0; 6][..], &less_one[..3], &less_one[..3], &[0; 4], &frame].concat();
            chunks.extend(riff_chunk(b"ANMF", &anmf));
        } else {
            chunks.extend(frame);
        }
        let covered = if riff_covers_frame {
            chunks.len()
        } else {
            before_frame
        };
        let riff_size = (4 + covered as u32).to_le_bytes();
        let file = [&b"RIFF"[..], &riff_size, b"WEBP", &chunks].concat();
        data_url("image/webp", &file)
    }

    #[test]
    fn a_webp_frame_past_the_caps_is_refused_whatever_the_canvas() {
        let mut encoded = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut encoded, ImageFormat::WebP)
            .unwrap();
        for href in [
            data_url("image/webp", encoded.get_ref()),
            webp_file(4, false, vp8(4, 4), true),
            webp_file(4, false, vp8l(4, 4), true),
            webp_file(4, true, vp8(4, 4), true),
            webp_file(4, false, vp8(4, 4), false),
            webp_file(4, true, vp8(4, 4), false),
        ] {
            assert!(shown(&href), "{href}");
        }
        let over: Vec<&str> = [
            (
                "VP8X 1x1 around VP8 8200",
                webp_file(1, false, vp8(8200, 8200), true),
            ),
            (
                "VP8X 1x1 around VP8L 8200",
                webp_file(1, false, vp8l(8200, 8200), true),
            ),
            (
                "ANMF frame VP8 8200",
                webp_file(1, true, vp8(8200, 8200), true),
            ),
            (
                "VP8 8200 past the RIFF end",
                webp_file(1, false, vp8(8200, 8200), false),
            ),
            (
                "ANMF VP8 8200 past the RIFF end",
                webp_file(1, true, vp8(8200, 8200), false),
            ),
        ]
        .into_iter()
        .filter(|(_, href)| shown(href))
        .map(|(name, _)| name)
        .collect();
        assert_eq!(over, Vec::<&str>::new());
    }

    fn gif_header(screen: u16, width: u16, height: u16) -> Vec<u8> {
        [
            &b"GIF89a"[..],
            &screen.to_le_bytes(),
            &screen.to_le_bytes(),
            &[0x80, 0, 0, 0, 0, 0, 255, 255, 255, 0x2c, 0, 0, 0, 0],
            &width.to_le_bytes(),
            &height.to_le_bytes(),
            &[0, 2, 0, 0x3b],
        ]
        .concat()
    }

    #[test]
    fn a_gif_first_frame_past_the_caps_is_refused_whatever_the_screen() {
        let gif = |screen, width, height| data_url("image/gif", &gif_header(screen, width, height));
        assert!(shown(&gif(4, 4, 4)));
        assert!(!shown(&gif(1, 65535, 190)));
    }
}
