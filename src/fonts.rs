//! The app's faces and their fallback chains, for the shell's own
//! screens and a guest view's text alike.

/// The app's two faces, the design canvas's: they stand in for the
/// `design` crate's everywhere the app draws, a guest view's text included
/// (`refine_fallbacks` maps the crate's names onto these).
pub(crate) const FAMILY_UI: &str = "Instrument Sans";
pub(crate) const FAMILY_MONO: &str = "IBM Plex Mono";

pub(crate) const BUNDLED_FACES: &[&[u8]] = &[
    include_bytes!("../assets/fonts/InstrumentSans-Regular.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-Italic.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-Medium.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-Bold.ttf"),
    include_bytes!("../assets/fonts/InstrumentSans-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Italic.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Bold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-BoldItalic.ttf"),
    include_bytes!("../assets/fonts/Pretendard-Regular.otf"),
    include_bytes!("../assets/fonts/Pretendard-Medium.otf"),
    include_bytes!("../assets/fonts/Pretendard-Bold.otf"),
    include_bytes!("../assets/fonts/D2Coding-Regular.ttf"),
    include_bytes!("../assets/fonts/D2Coding-Bold.ttf"),
];

#[cfg(not(target_os = "macos"))]
pub(crate) const EMOJI_FACE: &[u8] = include_bytes!("../assets/fonts/NotoColorEmoji.ttf");

pub(crate) const FALLBACK_FAMILIES: &[&str] = &[
    "Apple Color Emoji",
    "Noto Color Emoji",
    "Apple SD Gothic Neo",
    "Hiragino Sans",
    "PingFang SC",
    "Noto Sans",
    "DejaVu Sans",
    "Apple Symbols",
];

pub(crate) fn fallback_chain() -> gpui_kit::FontFallbacks {
    static CHAIN: std::sync::LazyLock<gpui_kit::FontFallbacks> =
        std::sync::LazyLock::new(|| chain_led_by(design::fonts::FAMILY_UI_HANGUL));
    CHAIN.clone()
}

pub(crate) fn mono_fallback_chain() -> gpui_kit::FontFallbacks {
    static CHAIN: std::sync::LazyLock<gpui_kit::FontFallbacks> =
        std::sync::LazyLock::new(|| chain_led_by(design::fonts::FAMILY_MONO_HANGUL));
    CHAIN.clone()
}

pub(crate) fn chain_led_by(hangul: &str) -> gpui_kit::FontFallbacks {
    gpui_kit::FontFallbacks::from_fonts(
        std::iter::once(hangul.to_string())
            .chain(FALLBACK_FAMILIES.iter().map(|name| name.to_string()))
            .collect(),
    )
}

/// The face the app draws for `family`: a guest names the design crate's
/// faces, and the app's own stand in for them.
pub(crate) fn app_family(family: &gpui_kit::SharedString) -> gpui_kit::SharedString {
    match family.as_ref() {
        design::fonts::FAMILY_UI => FAMILY_UI.into(),
        design::fonts::FAMILY_MONO => FAMILY_MONO.into(),
        _ => family.clone(),
    }
}

/// Pairs the family a guest asked for with the fallback chain that carries its
/// Hangul: code faces fall back to the monospace Hangul face, everything else
/// to the proportional one. Wire styles arrive by assignment, so this runs at
/// the assignment sites rather than through the Styled builder.
pub(crate) fn refine_fallbacks(style: &mut gpui_kit::StyleRefinement) {
    if let Some(family) = &mut style.text.font_family {
        *family = app_family(family);
    }
    let is_code_face = style
        .text
        .font_family
        .as_deref()
        .is_some_and(|family| family == FAMILY_MONO);
    style.text.font_fallbacks = Some(if is_code_face {
        mono_fallback_chain()
    } else {
        fallback_chain()
    });
}

#[cfg(all(test, target_os = "linux"))]
#[path = "tests/font_fallback.rs"]
mod font_fallback;
