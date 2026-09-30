//! Links and badges (desk.rs).

use super::{AppMessage as Message, Ducktape};
use crate::runtime::Intent;

#[test]
fn a_link_on_this_chain_hands_its_view_the_route() {
    let mut state = Ducktape::boot();
    let chain = "testkit#0a1b2c3d";
    state.roster = crate::runtime::Roster::listing(&["link-test-here", "link-test-away"]);
    let _ = state.open_link("duck://testkit-0a1b2c3d/link-test-here/tx/00ff", chain);
    assert_eq!(state.active, Some("link-test-here"));
    assert!(state.toast.is_empty(), "the view coming forward says it");
    assert_eq!(
        crate::runtime::take_route("link-test-here").as_deref(),
        Some("tx/00ff")
    );
    // another chain's link opens the seat, routes nothing, and says why
    let _ = state.open_link("duck://othernet-0a1b2c3d/link-test-away/tx/00ff", chain);
    assert_eq!(state.active, Some("link-test-away"));
    assert_eq!(crate::runtime::take_route("link-test-away"), None);
    assert!(state.toast.contains("othernet"));
    // a view nobody lists opens nothing
    state.toast.clear();
    let _ = state.open_link("duck://testkit-0a1b2c3d/link-test-nowhere/x", chain);
    assert_eq!(state.active, Some("link-test-away"));
    assert!(state.toast.contains("link-test-nowhere"));
}

#[test]
fn opening_a_view_keeps_its_badge_until_the_view_clears_it() {
    let mut state = Ducktape::boot();
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(3)));
    let _ = state.update(Message::SelectView("chat"));
    assert_eq!(state.badges.get("chat"), Some(&3));
}

#[test]
fn a_view_event_sets_its_badge_and_opens_its_link() {
    let mut state = Ducktape::boot();
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(3)));
    assert_eq!(state.badges.get("chat"), Some(&3));
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(0)));
    assert!(state.badges.is_empty(), "a zero count clears the badge");
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(-2)));
    assert!(state.badges.is_empty());
    let _ = state.update(Message::ViewEvent("chat", Intent::Notified));
    assert!(state.badges.is_empty() && state.active.is_none());

    state.roster = crate::runtime::Roster::listing(&["view-event-link"]);
    let console = crate::shell::WindowKey::unique();
    state.console_win = Some(console);
    // a view's link comes back as a message, for `Desktop::dispatch` to
    // read against the chain in hand
    let task = state.update(Message::ViewEvent(
        "chat",
        Intent::OpenLink("duck://view-event-link/room/7".into()),
    ));
    let Some(Message::OpenLink(link)) =
        futures::executor::block_on(futures::StreamExt::next(&mut task.into_stream()))
    else {
        panic!("a view's link did not come back as a message");
    };
    let _ = state.open_link(&link, "");
    assert_eq!(state.active, Some("view-event-link"));
    assert_eq!(
        state.layouts[&console].shown(),
        Some("view-event-link"),
        "a link brings its seat forward"
    );
    assert_eq!(
        crate::runtime::take_route("view-event-link").as_deref(),
        Some("room/7")
    );
}
