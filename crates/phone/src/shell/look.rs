//! The Expo build's look, for the chrome the host draws on a phone: its type
//! roles, fixed sizes and palette (mobile `src/design/theme.ts`,
//! `src/design/tokens.gen.ts`, dev 6f23151; the palette is the `design`
//! crate's, which the Expo tokens are generated from). Screens the Expo app
//! has are ported to these numbers exactly; screens it lacks (Home, the
//! capsule, Open) are drawn with them (design `fidelity/TARGET.md`).

use ducktape_host::a11y::Control as _;
use ducktape_host::fonts::FAMILY_UI;
use gpui_kit::{
    App, ClickEvent, Div, FontWeight, Hsla, InteractiveElement as _, ParentElement as _, Rgba,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb, svg,
};
use view_wire::manifest::Tint;

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
/// A row (`size.row`).
pub const ROW: f32 = 48.;
/// A row with a caption (`size.rowTall`).
pub const ROW_TALL: f32 = 56.;
/// A badge's height (`size.badge`).
pub const BADGE: f32 = 20.;
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
/// A sheet row (`size.sheetRow`).
pub const SHEET_ROW: f32 = 52.;
/// The icon in a sheet row (`size.icon.sheet`).
pub const ICON_SHEET: f32 = 20.;
/// A sheet's side inset (`Sheet.tsx`: 20).
pub const SHEET_INSET: f32 = 20.;
/// A pressable while it is held (Expo `platform.pressed`, the boards'
/// `.press`): dimmed to this opacity.
pub const PRESSED: f32 = 0.6;

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

/// The Expo `Header` of a page that is not a root (kit `Header.tsx`): 56
/// tall, the back hit `id` at the left, then what the page adds (its
/// title block, its right hits), on the background, a hairline under.
pub fn header(
    id: &'static str,
    back: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    palette: Palette,
) -> Div {
    let back = div()
        .id(id)
        .control(gpui_kit::Role::Button, "Back")
        .size(px(HIT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .active(|style| style.opacity(PRESSED))
        .child(
            svg()
                .path("icons/arrow-left.svg")
                .size(px(ICON_HIT))
                .text_color(palette.foreground()),
        )
        .on_click(back);
    div()
        .h(px(HEADER))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(gap::XXS))
        .px(px(gap::SM))
        .border_b_1()
        .border_color(palette.border())
        .bg(palette.background())
        .child(back)
}

/// The palette of the kit theme's mode: the appearance App settings chose,
/// or under System the system's (`shell::sync_appearance` sets the theme,
/// which the views and the host's own chrome read too).
pub fn palette(cx: &App) -> Palette {
    match gpui_kit::component::Theme::global(cx).is_dark() {
        true => Palette {
            colours: &design::DARK,
            dark: true,
        },
        false => Palette {
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

    /// A program's tile in the tint its view's manifest names (SPEC §7.7):
    /// its ground and its glyph's ink, in this appearance. The name is the
    /// program's, the colours are the app's: a soft ground under a strong
    /// ink, the glyph at 3:1 or more on its ground in both appearances, the
    /// design palette's own pairs where one fits (green is `success`, red
    /// `danger`, violet `agent`; `warning` is too near orange, so amber is
    /// its own, clearly yellow). No tint is the neutral filled tile.
    pub fn tint(self, tint: Tint) -> (Hsla, Hsla) {
        // [ground, ink], light then dark
        let pair = |light: [u32; 2], dark: [u32; 2]| {
            let [ground, ink] = if self.dark { dark } else { light };
            (rgb(ground).into(), rgb(ink).into())
        };
        match tint {
            Tint::None => (self.surface_raised(), self.foreground()),
            Tint::Blue => pair([0xD8EDFF, 0x1762B6], [0x182E4B, 0x99CEFF]),
            Tint::Teal => pair([0xCBF6EF, 0x00786C], [0x003732, 0x6AE2D4]),
            Tint::Green => (self.success_soft(), self.success()),
            Tint::Lime => pair([0xE4F1D1, 0x546E0D], [0x27330E, 0xBAD787]),
            Tint::Amber => pair([0xFDEFBA, 0x8D6C00], [0x3B2F04, 0xF2D76C]),
            Tint::Orange => pair([0xFFE3D1, 0xA34100], [0x45240F, 0xFFB589]),
            Tint::Red => (self.danger_soft(), self.danger()),
            Tint::Pink => pair([0xFFE0EF, 0x9E366F], [0x442032, 0xFFADD5]),
            Tint::Violet => (self.agent_soft(), self.agent()),
        }
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
    success,
    success_soft,
    danger,
    danger_soft,
    agent,
    agent_soft
);

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: Palette = Palette {
        colours: &design::LIGHT,
        dark: false,
    };
    const DARK: Palette = Palette {
        colours: &design::DARK,
        dark: true,
    };

    /// A tint is its own pair in each appearance, the design palette's
    /// where it has one; no tint is the neutral filled tile.
    #[test]
    fn a_tint_is_its_pair_and_none_is_the_neutral_tile() {
        assert_eq!(
            LIGHT.tint(Tint::Blue),
            (rgb(0xD8EDFF).into(), rgb(0x1762B6).into())
        );
        assert_eq!(
            DARK.tint(Tint::Blue),
            (rgb(0x182E4B).into(), rgb(0x99CEFF).into())
        );
        assert_eq!(DARK.tint(Tint::Violet), (DARK.agent_soft(), DARK.agent()));
        for palette in [LIGHT, DARK] {
            assert_eq!(
                palette.tint(Tint::None),
                (palette.surface_raised(), palette.foreground())
            );
        }
    }

    /// Every tile's glyph reads on its ground: 3:1 or more (WCAG's
    /// non-text contrast), in both appearances.
    #[test]
    fn every_glyph_reads_on_its_ground() {
        let luminance = |colour: Hsla| {
            let linear = |c: f32| match c <= 0.04045 {
                true => c / 12.92,
                false => ((c + 0.055) / 1.055).powf(2.4),
            };
            let Rgba { r, g, b, .. } = colour.to_rgb();
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        };
        for palette in [LIGHT, DARK] {
            for tint in Tint::ALL {
                let (ground, ink) = palette.tint(tint);
                let (a, b) = (luminance(ground), luminance(ink));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(ratio >= 3., "{tint:?} dark={}: {ratio:.2}", palette.dark);
            }
        }
    }
}
