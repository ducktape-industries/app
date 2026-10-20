//! A program's page (the shell boards' 21, wasm-mobile SPEC §7.2): what it
//! does and what it may do, before or while it runs. Its glyph, name and
//! the description its manifest gives, Open, then each capability its
//! manifest declares in the Expo app's words (`runtime::allows`), the
//! narrowest pointer window it takes and the programs it talks to. A page
//! pushed over the stack: the header's back arrow (or Back) leaves it, its
//! link icon copies the program's link.

use super::look::{self, Palette};
use super::{Root, home};
use ducktape_host::a11y::Control as _;
use ducktape_host::runtime::{self, Listing, RailRow};
use gpui_kit::{
    Context, Div, FontWeight, InteractiveElement as _, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px, svg,
};

/// The page of `row`'s program, its manifest `listing` once its seat has
/// read it.
pub fn page(
    row: &RailRow,
    listing: Option<Listing>,
    palette: Palette,
    cx: &mut Context<Root>,
) -> Div {
    let program = row.program;
    let back = cx.listener(|root, _, _, cx| root.pop_page(cx));
    let header = look::header("about/back", back, palette)
        .justify_between()
        .child(
            div()
                .id("about/copy-link")
                .control(Role::Button, "Copy link")
                .size(px(look::HIT))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("icons/link.svg")
                        .size(px(look::ICON_HIT))
                        .text_color(palette.foreground()),
                )
                .on_click(cx.listener(move |root, _, _, cx| root.copy_link(program, cx))),
        );
    let description = listing
        .as_ref()
        .map(|listing| listing.description.clone())
        .unwrap_or_else(|| row.description.clone());
    let mut words = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(look::gap::XXS))
        .child(
            look::text(look::HEADING, FontWeight::MEDIUM, palette.foreground())
                .child(home::name_of(row)),
        );
    if !description.is_empty() {
        words = words.child(
            look::text(look::SECONDARY, FontWeight::NORMAL, palette.muted()).child(description),
        );
    }
    let hero = div()
        .flex()
        .items_center()
        .gap(px(look::gap::LG))
        .p(px(look::GUTTER))
        .child(home::glyph(row, 56., 28., palette))
        .child(words);
    let open = div()
        .id("about/open")
        .control(Role::Button, "Open")
        .mx(px(look::GUTTER))
        .mb(px(look::GUTTER))
        .h(px(look::PRIMARY))
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.primary())
        .child(
            look::text(look::BODY, FontWeight::MEDIUM, palette.primary_foreground()).child("Open"),
        )
        .on_click(cx.listener(move |root, _, _, cx| {
            root.pop_page(cx);
            root.open(program, cx);
        }));
    let mut body = div()
        .id("about")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(hero)
        .child(open)
        .child(div().h(px(1.)).bg(palette.border()));
    if let Some(listing) = listing {
        let count = listing.capabilities.len();
        let counted = match count {
            1 => "1 capability".to_owned(),
            count => format!("{count} capabilities"),
        };
        body = body
            .child(section("Capabilities", Some(counted), palette))
            .children(listing.capabilities.iter().map(|capability| {
                fact(
                    capability.as_str(),
                    runtime::allows(*capability),
                    false,
                    palette,
                )
            }));
        body = body.child(section("About", None, palette)).child(fact(
            "Min width",
            &format!("{} px", listing.min_width),
            false,
            palette,
        ));
    }
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(header)
        .child(body)
}

pub(super) fn section(
    label: impl Into<SharedString>,
    count: Option<String>,
    palette: Palette,
) -> Div {
    div()
        .min_h(px(40.))
        .flex()
        .items_center()
        .justify_between()
        .pt(px(look::gap::LG))
        .pb(px(look::gap::XXS))
        .px(px(look::GUTTER))
        .child(look::text(look::CAPTION, FontWeight::MEDIUM, palette.muted()).child(label.into()))
        .children(count.map(|count| {
            look::text(look::CAPTION, FontWeight::NORMAL, palette.muted()).child(count)
        }))
}

fn fact(label: &str, value: &str, mono: bool, palette: Palette) -> Div {
    let mut shown = look::text(look::SMALL, FontWeight::NORMAL, palette.foreground())
        .flex_1()
        .min_w_0();
    if mono {
        shown = shown.font_family(ducktape_host::fonts::FAMILY_MONO);
    }
    div()
        .min_h(px(look::CONTROL))
        .flex()
        .items_start()
        .gap(px(look::gap::LG))
        .py(px(look::gap::MD))
        .px(px(look::GUTTER))
        .border_b_1()
        .border_color(palette.border())
        .child(
            look::text(look::SMALL, FontWeight::NORMAL, palette.muted())
                .w(px(96.))
                .flex_none()
                .child(SharedString::from(label.to_owned())),
        )
        .child(shown.child(SharedString::from(value.to_owned())))
}
