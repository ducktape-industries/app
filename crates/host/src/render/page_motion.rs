//! A view's page change, moved as the phone moves a screen. A view draws
//! only the page on top of its stack, marked with its depth
//! (`wire::Interactivity::page`, set by the SDK's `design::page`); the
//! first marked container of a frame is the page it shows. When a frame
//! shows another page at another depth, the host slides the new page in
//! over the old (deeper: a push) or the old off the new (shallower: a pop),
//! by the phone's own motion (`motion::stack`), while the old page is drawn
//! from the frame that had it, in a tree of its own that nothing hears and
//! that opens no device (its camera, microphone and web pieces empty). A
//! page at the same depth (a tab, a page put in place of another) is not
//! moved. A pointer window never moves, and its frames carry no mark.
//!
//! The page a push covered stays kept, at its depth, until a pop reaches
//! it: iOS's swipe back (the shell routes it here) drags the page on top
//! off the page kept under it, under the finger, as UIKit's stack does.
//! Let go short, the page slides back and the view hears nothing. Let go
//! as a Back the view hears, the page holds where the finger left it until
//! the view answers: a shallower page pops from there, any other answer
//! slides it back. Either starts at the speed the finger let go at. The
//! page under the finger is as it was left; the view's own frame takes its
//! place on the pop.
use super::*;
use crate::motion::{self, Motion, Slide, Tween};

/// The page a frame shows: its path, and how far over its stack's root.
pub(super) type Shown = (AuthoredPath, u32);

/// A page change under way.
pub(super) struct PageMotion {
    /// The page that was on top, in its own tree: drawn, never heard.
    /// Sliding back from a finger, the page kept under the one on top.
    leaving: Entity<ViewTree>,
    /// Whether the new page is shallower: the old slides off it.
    pop: bool,
    motion: Motion,
    /// Where it starts, 0 to 1: where a finger let the page go, else 0.
    from: f32,
    /// How fast it starts, its whole way per second: the finger's speed as
    /// it let go, else 0.
    velocity: f32,
    /// From the first frame that draws it, not the frame's arrival: a new
    /// page slow to lay out still moves its whole way.
    tween: Option<Tween>,
    slide: Slide,
}

/// iOS's swipe back on the page on top.
#[derive(Clone, Copy)]
pub(super) struct Finger {
    /// How far the page is dragged off, a share of the width.
    pub(super) progress: f32,
    /// How fast it moves off, shares of the width per second.
    pub(super) speed: f32,
    /// Let go as a Back the view hears: held there for its answer.
    pub(super) back: bool,
}

/// The page `root` shows: its first marked container.
pub(super) fn shown(root: &wire::Node) -> Option<Shown> {
    let mut page = None;
    commands::walk_authored_paths(root, None, &mut Vec::new(), &mut |node, path| {
        first(&mut page, node, path)
    });
    page
}

/// Keeps `node` as the page shown if it is marked and none is yet: a walk's
/// visit, in document order.
pub(super) fn first(page: &mut Option<Shown>, node: &wire::Node, path: &AuthoredPath) {
    if page.is_none()
        && let Some(depth) = page_depth(node)
    {
        *page = Some((path.clone(), depth));
    }
}

/// The depth a node says it is a page at.
fn page_depth(node: &wire::Node) -> Option<u32> {
    match node {
        wire::Node::Container(view_wire::ContainerNode {
            interactivity: Some(interactivity),
            ..
        }) => interactivity.page,
        _ => None,
    }
}

impl ViewTree {
    /// Takes the page the new frame shows (`page`) before the old frame
    /// goes: when it is at another depth, under a finger, the old page is
    /// kept to move off or under, with the scroll offsets it was left at;
    /// a push keeps it at its depth after, for the edge swipe. A pop
    /// drops what it uncovers and what was deeper, and starts where a
    /// finger let go. A page put in place at the same depth keeps all of
    /// it, a finger on it too.
    pub(super) fn turn_page(&mut self, page: Option<Shown>, cx: &mut Context<Self>) {
        let was = std::mem::replace(&mut self.page, page);
        let (Some((from, depth)), Some((_, to))) = (was, &self.page) else {
            self.page_motion = None;
            self.kept.clear();
            self.finger = None;
            return;
        };
        let to = *to;
        if depth == to || !crate::contract::os().facts().touch_first {
            return;
        }
        let pop = to < depth;
        self.kept.retain(|kept, _| *kept < to.min(depth));
        let (start, velocity) = match (pop, self.finger.take()) {
            (true, Some(finger)) => (finger.progress, finger.speed),
            _ => (0., 0.),
        };
        let mut leaving = None;
        commands::walk_authored_paths(&self.root, None, &mut Vec::new(), &mut |node, path| {
            if leaving.is_none() && *path == from && page_depth(node).is_some() {
                leaving = Some(node.clone());
            }
        });
        let Some(mut root) = leaving else {
            return;
        };
        // a copy drawn and never heard opens no device: a camera, a
        // microphone or a web page it opened would stay open, unseen, as
        // long as the page is kept. Each keeps its place, empty.
        root.for_each_mut(&mut |node| {
            if let wire::Node::Camera { id, style, .. }
            | wire::Node::Microphone { id, style, .. }
            | wire::Node::Web { id, style, .. } = node
            {
                *node = wire::Node::Container(view_wire::ContainerNode {
                    id: Some(id.clone()),
                    style: *style,
                    interactivity: None,
                    children: Vec::new(),
                });
            }
        });
        // the old tree is rooted at the page: its paths start at the page's
        // own id
        let under = from.len().saturating_sub(1);
        let theirs = |path: &AuthoredPath| path.starts_with(&from);
        let scrolls = self
            .scrolls
            .iter()
            .filter(|(path, _)| theirs(path))
            .map(|(path, handle)| (path[under..].to_vec(), handle.clone()))
            .collect();
        // a list is kept at the path its node carries, which the old tree
        // keeps: its state moves as it is (the new frame drops it anyway),
        // and a page pushed over itself starts its own list fresh
        let uniform_lists: HashMap<_, _> =
            self.uniform_lists.extract_if(|p, _| theirs(p)).collect();
        let variable_lists: HashMap<_, _> =
            self.variable_lists.extract_if(|p, _| theirs(p)).collect();
        let tree = Tree {
            root,
            styles: self.styles.clone(),
        };
        let (pictures, images, body) = (self.pictures.clone(), self.images.clone(), self.body);
        let leaving = cx.new(|_| {
            let mut tree = ViewTree::new(tree);
            tree.pictures = pictures;
            tree.images = images;
            tree.scrolls = scrolls;
            tree.uniform_lists = uniform_lists;
            tree.variable_lists = variable_lists;
            tree.body = body;
            tree
        });
        if !pop {
            self.kept.insert(depth, leaving.clone());
        }
        let (motion, slide) = motion::stack(pop, cx);
        self.page_motion = Some(PageMotion {
            leaving,
            pop,
            motion,
            from: start,
            velocity,
            tween: None,
            slide,
        });
    }

    /// iOS's swipe back, `progress` (a share of the width) in and moving
    /// at `speed` (shares per second): the page on top follows the finger
    /// off the page kept under it, and the finger moves it (no frame is
    /// asked for). A swipe begins only where [`Self::can_drag_page`] says.
    /// Whether the page follows: else the swipe is the shell's own, and its
    /// release a Back as ever.
    pub fn drag_page(&mut self, progress: f32, speed: f32, cx: &mut Context<Self>) -> bool {
        let progress = progress.clamp(0., 1.);
        match self.finger {
            // let go already, waiting for the view's answer
            Some(Finger { back: true, .. }) => return false,
            Some(_) => {}
            None if self.can_follow(cx) => {
                // a motion that has ended may not be cleared yet: it is over
                self.page_motion = None;
            }
            None => return false,
        }
        self.finger = Some(Finger {
            progress,
            speed,
            back: false,
        });
        cx.notify();
        true
    }

    /// Whether a swipe back beginning now drags the page on top: no finger
    /// holds it (one let go is still waiting for the view's answer), and
    /// the page is still, above the view's root with a page kept under it,
    /// with no sheet or dialog over it and motion not reduced.
    pub fn can_drag_page(&self, cx: &App) -> bool {
        self.finger.is_none() && self.can_follow(cx)
    }

    fn can_follow(&self, cx: &App) -> bool {
        let now = motion::now(cx);
        let still = self
            .page_motion
            .as_ref()
            .is_none_or(|moving| moving.tween.is_some_and(|tween| tween.done(now)));
        let covered = !self.sheets.is_empty() || !self.dialogs.is_empty();
        still && !covered && !motion::reduced(cx) && self.kept_under().is_some()
    }

    /// The page kept under the one on top.
    fn kept_under(&self) -> Option<&Entity<ViewTree>> {
        let (_, depth) = self.page.as_ref()?;
        self.kept.get(&depth.checked_sub(1)?)
    }

    /// The swipe let go short of Back, or as a Back the view did not hear:
    /// the page slides back over the one under it. A finger let go as a
    /// Back the view heard waits for the view's answer instead.
    pub fn drag_page_cancelled(&mut self, cx: &mut Context<Self>) {
        if self.finger.is_some_and(|finger| !finger.back) {
            self.slide_back(cx);
        }
    }

    /// The swipe let go as a Back the view hears: the page holds where the
    /// finger left it until the view answers.
    pub(crate) fn heard_back(&mut self) {
        if let Some(finger) = &mut self.finger {
            finger.back = true;
        }
    }

    /// The view answered the Back the swipe let go as, and no shallower
    /// page took the finger (it stayed, or the view drew nothing new): the
    /// page slides back.
    pub(crate) fn back_answered(&mut self, cx: &mut Context<Self>) {
        if self.finger.is_some_and(|finger| finger.back) {
            self.slide_back(cx);
        }
    }

    /// The page under the finger goes back into place over the one kept
    /// under it, from where the finger left it and at its speed, as a
    /// push's rest.
    fn slide_back(&mut self, cx: &mut Context<Self>) {
        let Some(finger) = self.finger.take() else {
            return;
        };
        cx.notify();
        // no travel: already in place, nothing to slide back
        let Some(under) = self.kept_under().filter(|_| finger.progress > 0.) else {
            return;
        };
        let (motion, slide) = motion::stack(false, cx);
        self.page_motion = Some(PageMotion {
            leaving: under.clone(),
            pop: false,
            motion,
            from: 1. - finger.progress,
            // back into place is the other way to the finger's
            velocity: -finger.speed,
            tween: None,
            slide,
        });
    }

    /// The page on top, `page` as drawn, moving over or under the page it
    /// replaced while that lasts, or off the page kept under it while a
    /// finger holds it; as drawn otherwise.
    pub(super) fn moving_page(
        &mut self,
        page: AnyElement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_top = self
            .page
            .as_ref()
            .is_some_and(|(path, _)| *path == self.authored_path);
        if !on_top {
            return page;
        }
        // where the screens are, the other page, and whether it is the one
        // on top
        let (placed, other, over) = if let Some(moving) = self.page_motion.as_mut() {
            let tween = *moving.tween.get_or_insert_with(|| {
                Tween::thrown(moving.from, 1., moving.velocity, moving.motion, cx)
            });
            let now = motion::now(cx);
            if tween.done(now) {
                self.page_motion = None;
                return page;
            }
            // it is still moving at `now`: one clock read for the frame
            // asked for and the place drawn
            window.request_animation_frame();
            let progress = tween.at(now);
            let placed = moving.slide.at(progress, tween.time(now), moving.pop);
            (placed, moving.leaving.clone(), moving.pop)
        } else if let Some(finger) = self.finger
            && let Some(under) = self.kept_under()
        {
            // the finger moves it: no frame asked for
            let (_, slide) = motion::stack(true, cx);
            let placed = slide.at(finger.progress, finger.progress, true);
            (placed, under.clone(), false)
        } else {
            return page;
        };
        // a press on the other page is no one's: over a pop it would
        // otherwise reach the page under it, unseen
        let other = div().size_full().occlude().child(other).into_any_element();
        let (top, under) = match over {
            true => (other, page),
            false => (page, other),
        };
        let ground = crate::theme::hsla(crate::theme::palette(cx).background);
        let screen = |left: f32, content: AnyElement| {
            div()
                .absolute()
                .top_0()
                .left(gpui_kit::relative(left))
                .size_full()
                .bg(ground)
                .child(content)
        };
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .child(screen(placed.under, under))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(rgb(0x000000))
                    .opacity(placed.dim),
            )
            .child(screen(placed.top, top).opacity(placed.opacity))
            .into_any_element()
    }

    /// Whether a page change (or a page sliding back) is moving.
    #[cfg(any(test, feature = "test-support"))]
    pub fn page_moving(&self) -> bool {
        self.page_motion.is_some()
    }

    /// How fast the page change under way started, its whole way per
    /// second.
    #[cfg(test)]
    pub(super) fn page_velocity(&self) -> Option<f32> {
        self.page_motion.as_ref().map(|moving| moving.velocity)
    }

    /// The tree the page going is drawn in, while it moves.
    #[cfg(test)]
    pub(super) fn leaving_page(&self) -> Option<Entity<ViewTree>> {
        self.page_motion
            .as_ref()
            .map(|moving| moving.leaving.clone())
    }

    /// Where the edge swipe holds the page on top: how far off, and
    /// whether let go as a Back waiting for the view's answer.
    #[cfg(any(test, feature = "test-support"))]
    pub fn page_finger(&self) -> Option<(f32, bool)> {
        self.finger.map(|finger| (finger.progress, finger.back))
    }
}
