//! Exercise the same Linux platform text system used by the native app.
use super::{BUNDLED_FACES, EMOJI_FACE, fallback_chain, mono_fallback_chain};
use gpui_kit::{FontRun, FontStyle, FontWeight, PlatformTextSystem, font, px};
use gpui_wgpu::CosmicTextSystem;
use std::borrow::Cow;

fn text_system() -> CosmicTextSystem {
    // No installed fonts: the application must be sufficient on a fresh Linux host.
    let system = CosmicTextSystem::new_without_system_fonts(design::fonts::FAMILY_UI);
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
    for family in [design::fonts::FAMILY_UI, design::fonts::FAMILY_MONO] {
        let system = text_system();
        let mut descriptor = font(family);
        descriptor.fallbacks = Some(if family == design::fonts::FAMILY_MONO {
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
    for (family, sample, italic) in [
        (design::fonts::FAMILY_UI, "Am", true),
        (design::fonts::FAMILY_MONO, "Am", true),
        (design::fonts::FAMILY_UI_HANGUL, "한글", false),
        (design::fonts::FAMILY_MONO_HANGUL, "한글", false),
    ] {
        let mut ids = Vec::new();
        for weight in [FontWeight::NORMAL, FontWeight::BOLD] {
            for style in [FontStyle::Normal, FontStyle::Italic]
                .into_iter()
                .take(if italic { 2 } else { 1 })
            {
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
