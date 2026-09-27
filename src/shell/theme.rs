//! The product theme on gpui-kit: its light and dark configs from the
//! design crate, the app's font families, scrollbars always shown. Also
//! the room macOS's traffic lights take (`traffic_lights`). The fonts
//! themselves live in `crate::fonts` and are registered by `launch`.

/// Loads the product theme (once) and makes it the kit's light and dark
/// theme, then re-applies the current mode.
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
    // Every scrollbar in the app shows, as a guest view's scroller shows its
    // own (`render::layout`): a height-capped composer with no scrollbar
    // reads as a message cut short. `Theme::change` projects this onto the
    // base theme on every switch.
    theme.scrollbar_mode = gpui_base::ScrollbarMode::Always;
    let mode = theme.mode;
    Theme::change(mode, None, cx);
}

/// The product theme config with the app's font families and the editor
/// background set from `palette`, over the kit's default highlight style.
pub(super) fn with_syntax_colors(
    product: &std::rc::Rc<gpui_kit::component::ThemeConfig>,
    defaults: &std::rc::Rc<gpui_kit::component::ThemeConfig>,
    palette: &design::Palette,
) -> std::rc::Rc<gpui_kit::component::ThemeConfig> {
    let mut theme = (**product).clone();
    theme.font_family = Some(FAMILY_UI.into());
    theme.mono_font_family = Some(FAMILY_MONO.into());
    let mut style = defaults.highlight.clone().unwrap_or_default();
    style.editor_background = Some(hsla_of(palette.background));
    theme.highlight = Some(style);
    std::rc::Rc::new(theme)
}

pub(super) fn hsla_of(color: design::Color) -> gpui_kit::Hsla {
    let [r, g, b, a] = color;
    gpui_kit::Rgba { r, g, b, a }.into()
}

/// The room macOS's traffic lights take at a title bar's left end, when
/// this window draws the bar itself (macOS, not fullscreen); `None`
/// elsewhere, where the system draws the title bar.
pub(super) fn traffic_lights(window: &gpui_kit::Window) -> Option<f32> {
    (cfg!(target_os = "macos") && !window.is_fullscreen()).then_some(78.)
}

pub(crate) use crate::fonts::{FAMILY_MONO, FAMILY_UI};

/// The scrollbars stay shown through a light/dark switch: the mode lives on
/// the kit theme, which every `Theme::change` projects onto the base theme
/// again.
#[cfg(test)]
#[gpui_kit::test]
fn every_bar_stays_shown_through_a_theme_switch(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::component::{Theme, ThemeMode};
    cx.update(|cx| {
        gpui_kit::init(cx);
        configure_native_theme(cx);
        let shown = |cx: &gpui_kit::App| gpui_base::Theme::global(cx).scrollbar.mode();
        assert_eq!(shown(cx), gpui_base::ScrollbarMode::Always);
        Theme::change(ThemeMode::Dark, None, cx);
        assert_eq!(shown(cx), gpui_base::ScrollbarMode::Always);
        Theme::sync_system_appearance(None, cx);
        assert_eq!(shown(cx), gpui_base::ScrollbarMode::Always);
    });
}
