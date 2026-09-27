//! Links, badges and what opens over the desk (desk.rs, overlay.rs).

use super::test_support::on_testkit;
use super::{AppMessage as Message, Ducktape, Overlay};
use crate::runtime::Intent;

#[test]
fn a_link_on_this_chain_hands_its_view_the_route() {
    let (mut state, _) = Ducktape::boot();
    state.chain = "testkit#0a1b2c3d".into();
    crate::runtime::list_for_test("link-test-here");
    crate::runtime::list_for_test("link-test-away");
    let _ = state.update(Message::OpenLink(
        "duck://testkit-0a1b2c3d/link-test-here/tx/00ff".into(),
    ));
    assert_eq!(state.active, Some("link-test-here"));
    assert!(state.toast.is_empty(), "the view coming forward says it");
    assert_eq!(
        crate::runtime::take_route("link-test-here").as_deref(),
        Some("tx/00ff")
    );
    // another chain's link opens the seat, routes nothing, and says why
    let _ = state.update(Message::OpenLink(
        "duck://othernet-0a1b2c3d/link-test-away/tx/00ff".into(),
    ));
    assert_eq!(state.active, Some("link-test-away"));
    assert_eq!(crate::runtime::take_route("link-test-away"), None);
    assert!(state.toast.contains("othernet"));
    // a view nobody lists opens nothing
    state.toast.clear();
    let _ = state.update(Message::OpenLink(
        "duck://testkit-0a1b2c3d/link-test-nowhere/x".into(),
    ));
    assert_eq!(state.active, Some("link-test-away"));
    assert!(state.toast.contains("link-test-nowhere"));
}

#[test]
fn opening_a_view_keeps_its_badge_until_the_view_clears_it() {
    let (mut state, _) = Ducktape::boot();
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(3)));
    let _ = state.update(Message::SelectView("chat"));
    assert_eq!(state.badges.get("chat"), Some(&3));
}

#[test]
fn a_view_event_sets_its_badge_and_opens_its_link() {
    let (mut state, _) = Ducktape::boot();
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(3)));
    assert_eq!(state.badges.get("chat"), Some(&3));
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(0)));
    assert!(state.badges.is_empty(), "a zero count clears the badge");
    let _ = state.update(Message::ViewEvent("chat", Intent::Badge(-2)));
    assert!(state.badges.is_empty());
    let _ = state.update(Message::ViewEvent("chat", Intent::Notified));
    assert!(state.badges.is_empty() && state.active.is_none());

    crate::runtime::list_for_test("view-event-link");
    let console = crate::shell::WindowKey::unique();
    state.console_win = Some(console);
    let _ = state.update(Message::ViewEvent(
        "chat",
        Intent::OpenLink("duck://view-event-link/room/7".into()),
    ));
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

#[test]
fn closing_an_overlay_lets_go_of_what_it_held() {
    use super::Popover;
    let mut state = on_testkit();
    let _ = state.update(Message::OpenSpotlight);
    let _ = state.update(Message::SpotlightTyped("chat".into()));
    let _ = state.update(Message::CloseOverlay(Overlay::Spotlight));
    assert!(state.overlay.is_none() && state.spotlight_query.is_empty());
    // Escape on the account menu closes it whichever menu is named
    let _ = state.update(Message::TogglePopover(Popover::Account));
    let _ = state.update(Message::CloseOverlay(Overlay::Menu(Popover::Node)));
    assert!(state.overlay.is_none());
    // picking a view from a menu closes the menu
    let _ = state.update(Message::TogglePopover(Popover::Account));
    let _ = state.update(Message::SelectView("module-registry"));
    assert!(state.overlay.is_none());
    // one overlay's close leaves another open
    let _ = state.update(Message::OpenSettings);
    let _ = state.update(Message::CloseOverlay(Overlay::Network));
    assert_eq!(state.overlay, Some(Overlay::Settings));
    let _ = state.update(Message::ApproveOpen);
    state.sign_in.unlock_error = "no such code".into();
    let _ = state.update(Message::CloseOverlay(Overlay::Approve));
    assert!(state.overlay.is_none() && state.sign_in.unlock_error.is_empty());
}
