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

impl Role {
    /// Its size in points.
    pub fn size(self) -> f32 {
        self.size
    }
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
/// small 14 / 19: a toast's words
pub const SMALL: Role = Role {
    size: 14.,
    line: 19.,
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
/// a detail head's name, 20 / 24
pub const HEADING: Role = Role {
    size: 20.,
    line: 24.,
};
/// a module root's header, 22 / 27
pub const BIG_TITLE: Role = Role {
    size: 22.,
    line: 27.,
};
/// a header's title, 17 / 21
pub const TITLE: Role = Role {
    size: 17.,
    line: 21.,
};
/// eyebrow 11 / 14, mono, upper case: the code box's label
pub const EYEBROW: Role = Role {
    size: 11.,
    line: 14.,
};
/// the join code, 36 / 44, mono
pub const DISPLAY: Role = Role {
    size: 36.,
    line: 44.,
};

/// The header (theme.ts `size.header`).
pub const HEADER: f32 = 56.;
/// A press target's side (`size.hit`).
pub const HIT: f32 = 44.;
/// A field, a plain button (`size.control`).
pub const CONTROL: f32 = 44.;
/// A primary button (`size.primary`).
pub const PRIMARY: f32 = 48.;
/// A small button (`size.small`).
pub const SMALL_BUTTON: f32 = 36.;
/// A row with a caption (`size.rowTall`).
pub const ROW_TALL: f32 = 56.;
/// A row's lead icon (`size.icon.row`).
pub const ICON_ROW: f32 = 18.;
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
/// A sheet row (`size.sheetRow`).
pub const SHEET_ROW: f32 = 52.;
/// The icon in a sheet row (`size.icon.sheet`).
pub const ICON_SHEET: f32 = 20.;
/// A sheet's side inset (`Sheet.tsx`: 20).
pub const SHEET_INSET: f32 = 20.;

/// The design crate's spacing ladder (`spacing`).
pub mod gap {
    /// `xxs`
    pub const XXS: f32 = 4.;
    /// `xs`
    pub const XS: f32 = 6.;
    /// `sm`
    pub const SM: f32 = 8.;
    /// `md`
    pub const MD: f32 = 10.;
    /// `lg`
    pub const LG: f32 = 12.;
    /// `block`
    pub const BLOCK: f32 = 16.;
    /// `xl`
    pub const XL: f32 = 24.;
}

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
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Palette {
            colours: &design::DARK,
            dark: true,
        },
        WindowAppearance::Light | WindowAppearance::VibrantLight => Palette {
            colours: &design::LIGHT,
            dark: false,
        },
    }
}

/// One appearance's colours, as gpui takes them.
#[derive(Clone, Copy)]
pub struct Palette {
    colours: &'static design::Palette,
    dark: bool,
}

impl Palette {
    /// Whether it is the dark appearance's.
    pub fn dark(self) -> bool {
        self.dark
    }

    /// The scrim under a sheet (theme.ts: black at 0.4, 0.55 in the dark).
    pub fn scrim(self) -> Hsla {
        let alpha = if self.dark { 0.55 } else { 0.4 };
        Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: alpha,
        }
        .into()
    }
}

macro_rules! colours {
    ($($name:ident),*) => {
        impl Palette {
            $(
                #[doc = concat!("`", stringify!($name), "`")]
                pub fn $name(self) -> Hsla {
                    let [r, g, b, a] = self.colours.$name;
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
    accent,
    primary,
    primary_foreground,
    danger,
    danger_soft
);
