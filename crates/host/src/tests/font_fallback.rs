//! Exercise the same Linux platform text system used by the native app.
#![cfg(target_os = "linux")]
use super::{BUNDLED_FACES, fallback_chain, mono_fallback_chain};
use gpui_kit::{FontRun, FontStyle, FontWeight, PlatformTextSystem, font, px};
use gpui_wgpu::CosmicTextSystem;
use std::borrow::Cow;

/// Noto's CBDT color font: the face the Linux desktop registers beside the
/// bundled ones (macOS supplies its own emoji; CoreGraphics cannot load it).
const EMOJI_FACE: &[u8] = include_bytes!("../../../../assets/fonts/NotoColorEmoji.ttf");

pub fn text_system() -> CosmicTextSystem {
    // No installed fonts: the application must be sufficient on a fresh Linux host.
    let system = CosmicTextSystem::new_without_system_fonts(super::FAMILY_UI);
    system
        .add_fonts(
            BUNDLED_FACES
                .iter()
                .copied()
                .chain([EMOJI_FACE])
                .map(Cow::Borrowed)
                .collect(),
        )
        .unwrap();
    system
}

#[test]
fn emoji_runs_resolve_to_bundled_color_face() {
    for family in [super::FAMILY_UI, super::FAMILY_MONO] {
        let system = text_system();
        let mut descriptor = font(family);
        descriptor.fallbacks = Some(if family == super::FAMILY_MONO {
            mono_fallback_chain()
        } else {
            fallback_chain()
        });
        // Resolve the app font first: load_family used to delete Noto here.
        let primary = system.font_id(&descriptor).unwrap();
        let emoji = system.font_id(&font("Noto Color Emoji")).unwrap();
        let text = "A😀📎🔊🟢한";
        let line = system.layout_line(
            text,
            px(24.),
            &[FontRun {
                len: text.len(),
                font_id: primary,
            }],
        );
        for (index, ch) in text
            .char_indices()
            .filter(|(_, ch)| "😀📎🔊🟢".contains(*ch))
        {
            let (run, glyph) = line
                .runs
                .iter()
                .find_map(|run| {
                    run.glyphs
                        .iter()
                        .find(|glyph| glyph.index == index)
                        .map(|glyph| (run, glyph))
                })
                .expect("emoji must produce a shaped glyph");
            assert_eq!(run.font_id, emoji, "{family}: {ch} selected another face");
            assert!(glyph.is_emoji, "{ch} must use the color rasterizer");
            assert_eq!(Some(glyph.id), system.glyph_for_char(emoji, ch));
            assert_ne!(glyph.id.0, 0, "{ch} must not be a missing glyph");
        }
    }
}

#[test]
fn every_bundled_static_face_is_selected_and_shapes() {
    let system = text_system();
    // `italic`: ships italics; `between`: ships Medium and SemiBold, upright.
    for (family, sample, italic, between) in [
        (super::FAMILY_UI, "Am", true, true),
        (super::FAMILY_MONO, "Am", true, true),
        (design::fonts::FAMILY_UI_HANGUL, "한글", false, true),
        (design::fonts::FAMILY_MONO_HANGUL, "한글", false, false),
    ] {
        let mut ids = Vec::new();
        let upright = [FontStyle::Normal].as_slice();
        let both = [FontStyle::Normal, FontStyle::Italic].as_slice();
        let ribbi = if italic { both } else { upright };
        let mut faces = vec![(FontWeight::NORMAL, ribbi), (FontWeight::BOLD, ribbi)];
        if between {
            faces.push((FontWeight::MEDIUM, upright));
            faces.push((FontWeight::SEMIBOLD, upright));
        }
        for (weight, styles) in faces {
            for &style in styles {
                let mut descriptor = font(family);
                descriptor.weight = weight;
                descriptor.style = style;
                let id = system.font_id(&descriptor).unwrap();
                assert!(!ids.contains(&id), "{family}: static face was substituted");
                ids.push(id);
                let line = system.layout_line(
                    sample,
                    px(24.),
                    &[FontRun {
                        len: sample.len(),
                        font_id: id,
                    }],
                );
                assert!(!line.runs.is_empty());
                for run in line.runs {
                    assert_eq!(run.font_id, id, "{family}: unexpected fallback");
                    assert!(run.glyphs.iter().all(|glyph| glyph.id.0 != 0));
                }
            }
        }
    }
}

/// A 600 run's Hangul falls back to Pretendard's own SemiBold: the fallback
/// is asked at the matched face's weight, so Hangul and Latin match.
#[test]
fn semibold_hangul_falls_back_to_its_own_weight() {
    let system = text_system();
    let mut descriptor = font(super::FAMILY_UI);
    descriptor.weight = FontWeight::SEMIBOLD;
    descriptor.fallbacks = Some(fallback_chain());
    let primary = system.font_id(&descriptor).unwrap();
    let mut hangul = font(design::fonts::FAMILY_UI_HANGUL);
    hangul.weight = FontWeight::SEMIBOLD;
    let hangul = system.font_id(&hangul).unwrap();
    let text = "A한";
    let line = system.layout_line(
        text,
        px(24.),
        &[FontRun {
            len: text.len(),
            font_id: primary,
        }],
    );
    let face_of = |index| {
        line.runs
            .iter()
            .find(|run| run.glyphs.iter().any(|glyph| glyph.index == index))
            .map(|run| run.font_id)
    };
    assert_eq!(face_of(0), Some(primary));
    assert_eq!(face_of(1), Some(hangul));
}
