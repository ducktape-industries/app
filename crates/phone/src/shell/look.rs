//! The Expo build's look, for the chrome the host draws on a phone: its type
//! roles, fixed sizes and palette (mobile `src/design/theme.ts`,
//! `src/design/tokens.gen.ts`, dev 6f23151; the palette is the `design`
//! crate's, which the Expo tokens are generated from). Screens the Expo app
//! has are ported to these numbers exactly; screens it lacks (Home, the
//! capsule, Open) are drawn with them (design `fidelity/TARGET.md`).

use ducktape_host::fonts::FAMILY_UI;
use gpui_kit::{Div, FontWeight, Hsla, Rgba, Styled as _, Window, WindowAppearance, div, px};

/// A text role: size and line height in points (theme.ts `type`).
#[derive(Clone, Copy)]
pub struct Role {
    size: f32,
    line: f32,
}

/// body 15 / 21
pub const BODY: Role = Role {
    size: 15.,
    line: 21.,
};
/// secondary 13 / 18
pub const SECONDARY: Role = Role {
    size: 13.,
    line: 18.,
};
/// caption 12 / 16
pub const CAPTION: Role = Role {
    size: 12.,
    line: 16.,
};
/// tiny 11 / 14: tab labels
pub const TINY: Role = Role {
    size: 11.,
    line: 14.,
};
/// a module root's header, 22 / 27
pub const BIG_TITLE: Role = Role {
    size: 22.,
    line: 27.,
};

/// The header (theme.ts `size.header`).
pub const HEADER: f32 = 56.;
/// A press target's side (`size.hit`).
pub const HIT: f32 = 44.;
/// The tab bar, over the bottom inset (`size.tabBar`).
pub const TAB_BAR: f32 = 56.;
/// A screen's side inset (`size.gutter` = spacing `block`).
pub const GUTTER: f32 = 16.;
/// The icon in a 44 hit (`size.icon.hit`).
pub const ICON_HIT: f32 = 22.;
/// An empty state's icon (`size.icon.empty`).
pub const ICON_EMPTY: f32 = 28.;
/// The avatar in a list or header (`size.avatar.list`).
pub const AVATAR: f32 = 32.;

/// Text in `role`, `weight`, `color`, in the app's face.
pub fn text(role: Role, weight: FontWeight, color: Hsla) -> Div {
    div()
        .font_family(FAMILY_UI)
        .text_size(px(role.size))
        .line_height(px(role.line))
        .font_weight(weight)
        .text_color(color)
}

/// The palette of the system's appearance: no in-app toggle (Expo
/// Settings → Appearance "Follows the system").
pub fn palette(window: &Window) -> Palette {
    match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Palette(&design::DARK),
        WindowAppearance::Light | WindowAppearance::VibrantLight => Palette(&design::LIGHT),
    }
}

/// One appearance's colours, as gpui takes them.
#[derive(Clone, Copy)]
pub struct Palette(&'static design::Palette);

macro_rules! colours {
    ($($name:ident),*) => {
        impl Palette {
            $(
                #[doc = concat!("`", stringify!($name), "`")]
                pub fn $name(self) -> Hsla {
                    let [r, g, b, a] = self.0.$name;
                    Rgba { r, g, b, a }.into()
                }
            )*
        }
    };
}

colours!(
    background,
    surface_raised,
    border,
    border_strong,
    foreground,
    muted,
    faint,
    accent
);
