//! Accessibility helpers for the native screens and the tree presenter: role and name, keyboard
//! reach, states gpui has no setter for, one AT node for a kit field, and a view cached only
//! while no assistive technology reads it (`cached`): gpui replays no node from a reused cache.

use gpui_kit::accesskit::{AriaCurrent, CustomAction, HasPopup, Invalid, Live, Orientation};
use gpui_kit::{
    AccessibleAction, App, Div, ElementId, FocusHandle, Hsla, InteractiveElement, Interactivity,
    IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, ParentElement, Role, SharedString,
    Stateful, StatefulInteractiveElement, Styled, WeakFocusHandle, Window, div,
};

/// A clickable element as assistive technology meets it: in the tree as
/// `role`, called `name`; the inspector reads it so too ([`inspected`]).
pub trait Control: StatefulInteractiveElement + ParentElement {
    fn control(self, role: Role, name: impl Into<SharedString>) -> Self {
        let name = name.into();
        let native = || Native {
            role: Some(role),
            label: Some(name.clone()),
            ..Default::default()
        };
        inspected(self, native).role(role).aria_label(name)
    }
}

impl<E: StatefulInteractiveElement + ParentElement> Control for E {}

/// What typing into a native field calls: the text, as SetValue gives it.
pub type SetValue = std::rc::Rc<dyn Fn(String, &mut Window, &mut App)>;

/// What the inspector reads of one of the host's own elements: its role,
/// its name, its text (none for a masked or private field), and for a
/// field what typing calls.
#[derive(Clone, Default)]
pub struct Native {
    pub role: Option<Role>,
    pub label: Option<SharedString>,
    pub value: Option<String>,
    pub set_value: Option<SetValue>,
}

/// `element` as the inspector reads it: each draw while the door serves
/// records what `native` says of it (`ax::natives`), under its own id.
#[cfg(feature = "ax-door")]
pub fn inspected<E: ParentElement>(element: E, native: impl FnOnce() -> Native) -> E {
    element.child(crate::ax::natives::mark(native()))
}

/// `element` as it is: a build with no door has no inspector to read it.
#[cfg(not(feature = "ax-door"))]
pub fn inspected<E: ParentElement>(element: E, _: impl FnOnce() -> Native) -> E {
    element
}

/// A control a keyboard reaches with Tab and presses with Enter or Space, as
/// a pointer presses it. A pointer's press does not move focus to it, so
/// pressing it leaves the caret where it was. Reached by Tab, it shows it
/// in `color` (see [`focus_shown`]).
pub fn keyboard<E: StatefulInteractiveElement>(element: E, color: Hsla) -> E {
    focus_shown(element.focusable().tab_stop(true), color)
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

/// The focus ring: 2px inside the edge, in `color` — the palette's ink, or
/// on an ink fill the ink's opposite (`Ink::ring` in the shell) — which reads
/// over a filled button as over a bare word, and moves nothing.
pub fn ring(color: Hsla) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow {
        color,
        offset: gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
        blur_radius: gpui_kit::px(0.),
        spread_radius: gpui_kit::px(2.),
        inset: true,
    }
}

/// The ink of the kit theme: the ring's colour where no shell palette is in
/// hand (a view's text field, a link).
pub fn ink(cx: &App) -> Hsla {
    gpui_kit::base::Theme::global(cx).tokens.colors.ring
}

/// The mark a control reached by the keyboard wears: the [`ring`] in
/// `color`, and the border in `color` too — gpui paints a border after an
/// inset shadow, so a 1px border in another colour would leave 1px of ring.
pub fn focus_shown<E: InteractiveElement>(element: E, color: Hsla) -> E {
    element.focus_visible(move |style| style.shadow(vec![ring(color)]).border_color(color))
}

/// A composite the keyboard meets as one Tab stop, WAI-ARIA APG's roving
/// tabindex (the shell's tab lists and radio groups): the item `active` of
/// `count` tracks `stop` ([`roving_item`]); the arrows along
/// `orientation`, wrapping at the ends, and Home and End tell `pick` the
/// item they make active, and the next draw hands `stop` to it, so the
/// keys follow. Unmodified keys only. Assistive technology's Focus on the
/// composite puts the keys on its active item. With no item there is
/// nothing to hand the keys to, and the composite takes none.
pub fn roving<E: StatefulInteractiveElement>(
    element: E,
    stop: &FocusHandle,
    orientation: Orientation,
    [active, count]: [usize; 2],
    pick: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> E {
    let element = element.aria_orientation(orientation);
    if count == 0 {
        return element;
    }
    let stop = stop.clone();
    // the active item as of the last key, ahead of the draw that shows it
    let at = std::cell::Cell::new(active);
    let (back, next) = match orientation {
        Orientation::Horizontal => ("left", "right"),
        Orientation::Vertical => ("up", "down"),
    };
    element
        .on_a11y_action(AccessibleAction::Focus, move |_, window, cx| {
            stop.focus(window, cx)
        })
        .on_key_down(move |event, window, cx| {
            let from = at.get();
            let to = match event.keystroke.key.as_str() {
                _ if event.keystroke.modifiers.modified() => return,
                key if key == next => (from + 1) % count,
                key if key == back => (from + count - 1) % count,
                "home" => 0,
                "end" => count - 1,
                _ => return,
            };
            cx.stop_propagation();
            if to != from {
                at.set(to);
                pick(to, window, cx);
            }
        })
}

/// An item of a [`roving`] composite: the active one tracks `stop`, the
/// composite's one Tab stop, and shows it in `color` ([`focus_shown`]);
/// the others take no focus.
pub fn roving_item<E: InteractiveElement>(
    element: E,
    stop: Option<&FocusHandle>,
    color: Hsla,
) -> E {
    match stop {
        Some(stop) => focus_shown(element.track_focus(stop), color),
        None => element,
    }
}

/// The accessibility setters of any interactive element, kit widgets that
/// do not expose them included. A widget still draws its own role and name
/// over these when it renders; the states it leaves alone stay as set here.
pub fn aria<E: InteractiveElement>(mut element: E, set: impl FnOnce(Aria<'_>) -> Aria<'_>) -> E {
    set(Aria(element.interactivity()));
    element
}

/// The node properties gpui has no setter for, written onto the element's
/// own node from its one `a11y_synthetic_children` closure. An element has
/// room for one closure — a second replaces the first — so everything one
/// element needs goes into one `Patch`. gpui runs it only for an element
/// with an id and a role. The view's aria mapper (`view_aria`) fills the
/// fields a view sets directly.
#[derive(Clone, Default, PartialEq)]
pub struct Patch {
    pub modal: bool,
    pub live: Option<Live>,
    pub busy: bool,
    pub disabled: bool,
    pub required: bool,
    pub read_only: bool,
    pub invalid: Option<Invalid>,
    pub has_popup: Option<HasPopup>,
    pub current: Option<AriaCurrent>,
    /// `(id, description)`, each requested as `Action::CustomAction`.
    pub custom_actions: Vec<(i32, String)>,
    pub class_name: Option<&'static str>,
    pub fallback: bool,
}

impl Patch {
    /// A modal dialog boundary.
    pub fn modal(self) -> Self {
        Self {
            modal: true,
            ..self
        }
    }

    /// A live region: a change to its words is announced, `Polite` after
    /// what is being read, `Assertive` at once.
    pub fn live(self, live: Live) -> Self {
        Self {
            live: Some(live),
            ..self
        }
    }

    pub fn disabled(self) -> Self {
        Self {
            disabled: true,
            ..self
        }
    }

    /// Its form refuses it empty.
    pub fn required(self) -> Self {
        Self {
            required: true,
            ..self
        }
    }

    /// A field drawn with an error; its description says which.
    pub fn invalid(self) -> Self {
        Self {
            invalid: Some(Invalid::True),
            ..self
        }
    }

    /// What its press opens: a menu, a dialog.
    pub fn has_popup(self, popup: HasPopup) -> Self {
        Self {
            has_popup: Some(popup),
            ..self
        }
    }

    /// Work it started is in flight ("Creating…").
    pub fn busy(self) -> Self {
        Self { busy: true, ..self }
    }

    /// One of the door's classes, [`AX_PRIVATE`] or [`AX_WHOLE`].
    pub fn class_name(self, class_name: &'static str) -> Self {
        Self {
            class_name: Some(class_name),
            ..self
        }
    }

    /// A box that holds the keys only when nothing inside it does (a
    /// window's root): the app focuses it, Tab never lands on it, so it
    /// offers assistive technology no focus the keyboard cannot reach
    /// either. gpui offers one on every element that tracks a handle.
    pub fn keys_fallback(self) -> Self {
        Self {
            fallback: true,
            ..self
        }
    }

    /// Installs the one closure on `element`.
    pub fn on<E: InteractiveElement>(self, element: E) -> E {
        if self == Self::default() {
            return element;
        }
        aria(element, |node| {
            node.a11y_synthetic_children(move |tree| {
                let node = tree.parent_node();
                if self.modal {
                    node.set_modal();
                }
                if let Some(live) = self.live {
                    node.set_live(live);
                }
                if self.busy {
                    node.set_busy();
                }
                if self.disabled {
                    node.set_disabled();
                }
                if self.required {
                    node.set_required();
                }
                if self.read_only {
                    // its text is not changed, so it offers no change
                    node.set_read_only();
                    node.remove_action(gpui_kit::accesskit::Action::SetValue);
                }
                if let Some(invalid) = self.invalid {
                    node.set_invalid(invalid);
                }
                if let Some(popup) = self.has_popup {
                    node.set_has_popup(popup);
                }
                if let Some(current) = self.current {
                    node.set_aria_current(current);
                }
                if !self.custom_actions.is_empty() {
                    node.set_custom_actions(
                        self.custom_actions
                            .into_iter()
                            .map(|(id, description)| CustomAction { id, description })
                            .collect::<Vec<_>>(),
                    );
                }
                if let Some(class_name) = self.class_name {
                    node.set_class_name(class_name);
                }
                if self.fallback {
                    node.remove_action(gpui_kit::accesskit::Action::Focus);
                }
            })
        })
    }
}

/// Marks the element's accessible node as a modal dialog boundary.
pub fn modal<E: InteractiveElement>(element: E) -> E {
    Patch::default().modal().on(element)
}

/// Words assistive technology announces as they appear and change: the
/// element's name and its value (AT-SPI, UIA and, since accesskit_macos
/// 0.27, macOS speak the name; a `Label`-role node takes its name from its
/// value), in a live region. The caller draws the words as bare text, so
/// they are read once.
pub fn live<E: StatefulInteractiveElement>(
    element: E,
    live: Live,
    words: impl Into<SharedString>,
) -> E {
    let words = words.into();
    Patch::default()
        .live(live)
        .on(element.aria_label(words.clone()).aria_value(words))
}

/// An element's accessibility properties, reached through its interactivity.
pub struct Aria<'a>(&'a mut Interactivity);

impl InteractiveElement for Aria<'_> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.0
    }
}

impl StatefulInteractiveElement for Aria<'_> {}

/// A text field as assistive technology meets it: one node, `id`, that Tab
/// and the Focus action both land on. The kit's text inputs keep their tab
/// stop on an inner element with no role, so this node tracks the text's own
/// `focus` handle and hands SetValue to `set_value`. The caller draws `field`
/// with no node of its own and sets the role, the name and the states here.
/// It is not the field's box, which is drawn around the text (a bare field)
/// or is the kit's or the view's own, so it wears no ring: the box does,
/// through [`around_field`].
pub fn text_field(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
    field: impl IntoElement,
) -> Stateful<Div> {
    typed(div().id(id).w_full().track_focus(focus), set_value).child(field)
}

/// The box of a [`text_field`] while the field is focused (`focused`),
/// however it got there — a caret alone is too faint a mark for where typing
/// goes: it wears the [`ring`] in `color`, and the border too (see
/// [`focus_shown`]).
pub fn around_field<E: Styled>(element: E, focused: bool, color: Hsla) -> E {
    match focused {
        true => element.shadow(vec![ring(color)]).border_color(color),
        false => element,
    }
}

/// An editable combo box as assistive technology meets it (a field and the
/// list it picks from): one node, `id`, around the field and its list, that
/// holds the field's `focus`, so the list's picked row can claim to be its
/// active descendant — gpui honours the claim only under the focused node.
/// The field inside is a [`text_field`] with no role: it keeps Tab, and has
/// no node of its own. The caller sets the name, the value and
/// whether the list shows (`aria_expanded`).
pub fn combo_box(
    id: impl Into<ElementId>,
    focus: &FocusHandle,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    typed(
        div().id(id).role(Role::EditableComboBox).track_focus(focus),
        set_value,
    )
}

/// `element` hands SetValue's text to `set_value`.
fn typed(
    element: Stateful<Div>,
    set_value: impl Fn(String, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    element.on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
        if let Some(gpui_kit::accesskit::ActionData::Value(value)) = data {
            set_value(value.to_string(), window, cx);
        }
    })
}

/// The class of a node whose value is private, and its name too unless it
/// is a text field (a field's name is its label): the app's test door masks
/// them before they leave the process. Assistive technology still reads
/// them — they are on the screen.
pub const AX_PRIVATE: &str = "ax_private";

/// Class name of a node whose name the AX tree gives whole, past its
/// length cap: a code an agent must read back in full.
pub const AX_WHOLE: &str = "ax_whole";

/// Marks the element's accessible node [`AX_WHOLE`]: a code or fingerprint
/// compared by eye.
pub fn whole<E: InteractiveElement>(element: E) -> E {
    Patch::default().class_name(AX_WHOLE).on(element)
}

/// Marks the element's accessible node [`AX_PRIVATE`]: the recovery phrase.
pub fn private<E: InteractiveElement>(element: E) -> E {
    Patch::default().class_name(AX_PRIVATE).on(element)
}

type Downs = Vec<(String, Option<WeakFocusHandle>)>;

#[derive(Clone, Default)]
pub struct HeldKeys(std::rc::Rc<std::cell::RefCell<Downs>>);

impl HeldKeys {
    pub fn on<E: InteractiveElement>(&self, element: E) -> E {
        let (downs, ups) = (self.0.clone(), self.0.clone());
        element
            .capture_key_down(move |event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke.key;
                if key != "enter" && key != "space" {
                    return;
                }
                let focus = window.focused(cx).map(|handle| handle.downgrade());
                let mut downs = downs.borrow_mut();
                let began = downs.iter().position(|(down, _)| down == key);
                match (event.is_held, began) {
                    (false, Some(at)) => downs[at].1 = focus,
                    (false, None) => downs.push((key.clone(), focus)),
                    (true, Some(at)) if downs[at].1 != focus => window.prevent_default(),
                    (true, _) => {}
                }
            })
            .capture_key_up(move |event: &KeyUpEvent, _, _| {
                ups.borrow_mut()
                    .retain(|(down, _)| *down != event.keystroke.key);
            })
    }
}

pub fn cached(view: gpui_kit::AnyView, style: gpui_kit::StyleRefinement) -> Cached {
    Cached {
        view,
        style,
        drawn: None,
    }
}

pub struct Cached {
    view: gpui_kit::AnyView,
    style: gpui_kit::StyleRefinement,
    drawn: Option<gpui_kit::AnyElement>,
}

impl IntoElement for Cached {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui_kit::Element for Cached {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui_kit::LayoutId, ()) {
        let view = self.view.clone();
        let mut drawn = match window.is_a11y_active() {
            true => {
                let mut held = div();
                *held.style() = self.style.clone();
                held.child(view).into_any_element()
            }
            false => view.cached(self.style.clone()).into_any_element(),
        };
        let layout = drawn.request_layout(window, cx);
        self.drawn = Some(drawn);
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        _bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(drawn) = &mut self.drawn {
            drawn.prepaint(window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&gpui_kit::GlobalElementId>,
        _inspector_id: Option<&gpui_kit::InspectorElementId>,
        _bounds: gpui_kit::Bounds<gpui_kit::Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(drawn) = &mut self.drawn {
            drawn.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ducktape_a11y_platform::headless::Harness;
    use gpui_kit::{Context, Render, TestAppContext, VisualTestContext, px, size};

    struct Patched(FocusHandle);

    impl Render for Patched {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            Patch::default()
                .modal()
                .live(Live::Polite)
                .class_name(AX_PRIVATE)
                .keys_fallback()
                .on(div()
                    .id("patched")
                    .role(Role::Dialog)
                    .aria_label("Patched")
                    .track_focus(&self.0))
        }
    }

    /// Everything one element needs lands on its node: a live region set
    /// beside a modal does not replace it.
    #[test]
    fn one_patch_writes_every_property_it_collects() {
        let native = Harness::open_with(gpui_kit::init, size(px(200.), px(200.)), |_, cx| {
            Patched(cx.focus_handle())
        });
        native.update(|window, cx| {
            native.activate_in(window);
            crate::render::draw_frame(window, cx);
            crate::render::draw_frame(window, cx);
            let update = &native.tree();
            let (_, node) = update
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some("Patched"))
                .unwrap();
            assert!(node.is_modal());
            assert_eq!(node.live(), Some(Live::Polite));
            assert_eq!(node.class_name(), Some(AX_PRIVATE));
            assert!(!node.supports_action(gpui_kit::accesskit::Action::Focus));
        });
    }

    struct Two {
        held: HeldKeys,
        a: FocusHandle,
        b: FocusHandle,
        clicks: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
    }

    impl Render for Two {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let button = |name: &'static str, focus: &FocusHandle| {
                let clicks = self.clicks.clone();
                div()
                    .id(name)
                    .size(px(20.))
                    .track_focus(focus)
                    .on_click(move |_, _, _| clicks.borrow_mut().push(name))
            };
            self.held.on(div()
                .size_full()
                .child(button("a", &self.a))
                .child(button("b", &self.b)))
        }
    }

    fn two(cx: &mut TestAppContext) -> (gpui_kit::Entity<Two>, VisualTestContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(200.), px(200.)), |_, cx| Two {
            held: HeldKeys::default(),
            a: cx.focus_handle(),
            b: cx.focus_handle(),
            clicks: Default::default(),
        });
        let view = window.root(cx).unwrap();
        let native = VisualTestContext::from_window(window.into(), cx);
        (view, native)
    }

    fn focus(view: &gpui_kit::Entity<Two>, native: &mut VisualTestContext, a: bool) {
        native.update(|window, cx| {
            let handle = view.read_with(cx, |two, _| match a {
                true => two.a.clone(),
                false => two.b.clone(),
            });
            handle.focus(window, cx);
            crate::render::draw_frame(window, cx);
        });
    }

    fn enter(native: &mut VisualTestContext, down: bool, is_held: bool) {
        let keystroke = gpui_kit::Keystroke::parse("enter").unwrap();
        match down {
            true => native.simulate_event(KeyDownEvent {
                keystroke,
                is_held,
                prefer_character_input: false,
            }),
            false => native.simulate_event(KeyUpEvent { keystroke }),
        }
    }

    #[gpui_kit::test]
    fn a_key_held_from_before_the_focus_moved_clicks_nothing(cx: &mut TestAppContext) {
        let (view, mut native) = two(cx);
        focus(&view, &mut native, false);
        enter(&mut native, true, false);
        focus(&view, &mut native, true);
        for _ in 0..3 {
            enter(&mut native, true, true);
        }
        enter(&mut native, false, false);
        let clicks =
            native.update(|_, cx| view.read_with(cx, |two, _| two.clicks.borrow().clone()));
        assert!(clicks.is_empty(), "{clicks:?}");
    }

    #[gpui_kit::test]
    fn a_press_held_on_its_element_clicks_it_once(cx: &mut TestAppContext) {
        let (view, mut native) = two(cx);
        focus(&view, &mut native, true);
        enter(&mut native, true, false);
        for _ in 0..3 {
            enter(&mut native, true, true);
        }
        enter(&mut native, false, false);
        let clicks =
            native.update(|_, cx| view.read_with(cx, |two, _| two.clicks.borrow().clone()));
        assert_eq!(clicks, ["a"]);
    }
}
