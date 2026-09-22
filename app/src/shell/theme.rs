// ---------- theme and fonts ----------

pub(super) fn configure_native_theme(cx: &mut gpui_kit::App) {
    use gpui_kit::component::{Theme, ThemeRegistry};
    let registry = ThemeRegistry::global_mut(cx);
    let registered = registry.themes().contains_key(design::LIGHT_THEME);
    if !registered {
        registry
            .load_themes_from_str(&design::kit_theme_json())
            .expect("the product theme parses");
    }
    let light = with_syntax_colors(
        &registry.themes()[design::LIGHT_THEME],
        registry.default_light_theme(),
        &design::LIGHT,
    );
    let dark = with_syntax_colors(
        &registry.themes()[design::DARK_THEME],
        registry.default_dark_theme(),
        &design::DARK,
    );
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    let mode = theme.mode;
    Theme::change(mode, None, cx);
}

pub(super) fn with_syntax_colors(
    product: &std::rc::Rc<gpui_kit::component::ThemeConfig>,
    defaults: &std::rc::Rc<gpui_kit::component::ThemeConfig>,
    palette: &design::Palette,
) -> std::rc::Rc<gpui_kit::component::ThemeConfig> {
    let mut theme = (**product).clone();
    let mut style = defaults.highlight.clone().unwrap_or_default();
    style.editor_background = Some(hsla_of(palette.background));
    theme.highlight = Some(style);
    std::rc::Rc::new(theme)
}

pub(super) fn hsla_of(color: design::Color) -> gpui_kit::Hsla {
    let [r, g, b, a] = color;
    gpui_kit::Rgba { r, g, b, a }.into()
}

pub(super) const RAIL_WIDTH: f32 = 200.;

pub(super) const BUNDLED_FACES: &[&[u8]] = &[
    include_bytes!("../../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/Inter-Italic.ttf"),
    include_bytes!("../../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../../assets/fonts/Inter-BoldItalic.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../../assets/fonts/JetBrainsMono-BoldItalic.ttf"),
    include_bytes!("../../assets/fonts/Pretendard-Regular.otf"),
    include_bytes!("../../assets/fonts/Pretendard-Bold.otf"),
    include_bytes!("../../assets/fonts/D2Coding-Regular.ttf"),
    include_bytes!("../../assets/fonts/D2Coding-Bold.ttf"),
];

#[cfg(not(target_os = "macos"))]
pub(super) const EMOJI_FACE: &[u8] = include_bytes!("../../assets/fonts/NotoColorEmoji.ttf");

pub(super) const FALLBACK_FAMILIES: &[&str] = &[
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

pub(super) fn chain_led_by(hangul: &str) -> gpui_kit::FontFallbacks {
    gpui_kit::FontFallbacks::from_fonts(
        std::iter::once(hangul.to_string())
            .chain(FALLBACK_FAMILIES.iter().map(|name| name.to_string()))
            .collect(),
    )
}

/// Pairs the family a guest asked for with the fallback chain that carries its
/// Hangul: code faces fall back to the monospace Hangul face, everything else
/// to the proportional one. Wire styles arrive by assignment, so this runs at
/// the assignment sites rather than through the Styled builder.
pub(crate) fn refine_fallbacks(style: &mut gpui_kit::StyleRefinement) {
    let is_code_face = style
        .text
        .font_family
        .as_deref()
        .is_some_and(|family| family == design::fonts::FAMILY_MONO);
    style.text.font_fallbacks = Some(if is_code_face {
        mono_fallback_chain()
    } else {
        fallback_chain()
    });
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../tests/font_fallback.rs"]
mod font_fallback;
