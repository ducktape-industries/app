//! A view's page change, moved as the phone moves a screen. A view draws
//! only the page on top of its stack, marked with its depth
//! (`wire::Interactivity::page`, set by the SDK's `design::page`); the
//! first marked container of a frame is the page it shows. When a frame
//! shows another page at another depth, the host slides the new page in
//! over the old (deeper: a push) or the old off the new (shallower: a pop),
//! by the phone's own motion (`motion::stack`), while the old page is drawn
//! from the frame that had it, in a tree of its own that nothing hears. A
//! page at the same depth (a tab, a page put in place of another) is not
//! moved. A pointer window never moves, and its frames carry no mark.
use super::*;
use crate::motion::{self, Slide, Tween};

/// The page a frame shows: its path, and how far over its stack's root.
pub(super) type Shown = (AuthoredPath, u32);

/// A page change under way.
pub(super) struct PageMotion {
    /// The page that was on top, in its own tree: drawn, never heard.
    leaving: Entity<ViewTree>,
    /// Whether the new page is shallower: the old slides off it.
    pop: bool,
    tween: Tween,
    slide: Slide,
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
    /// kept to move off or under, with the scroll offsets it was left at.
    pub(super) fn turn_page(&mut self, page: Option<Shown>, cx: &mut Context<Self>) {
        let was = std::mem::replace(&mut self.page, page);
        let (Some((from, depth)), Some((_, to))) = (was, &self.page) else {
            self.page_motion = None;
            return;
        };
        if depth == *to || !crate::contract::os().facts().touch_first {
            return;
        }
        let mut leaving = None;
        commands::walk_authored_paths(&self.root, None, &mut Vec::new(), &mut |node, path| {
            if leaving.is_none() && *path == from && page_depth(node).is_some() {
                leaving = Some(node.clone());
            }
        });
        let Some(root) = leaving else {
            return;
        };
        // the old tree is rooted at the page: its paths start at the page's
        // own id
        let under = from.len().saturating_sub(1);
        let scrolls = self
            .scrolls
            .iter()
            .filter(|(path, _)| path.starts_with(&from))
            .map(|(path, handle)| (path[under..].to_vec(), handle.clone()))
            .collect();
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
            tree.body = body;
            tree
        });
        let pop = *to < depth;
        let (moving, slide) = motion::stack(pop, cx);
        self.page_motion = Some(PageMotion {
            leaving,
            pop,
            tween: Tween::start(moving, cx),
            slide,
        });
    }

    /// The page on top, `page` as drawn, moving over or under the page it
    /// replaced while that lasts; as drawn once it is done.
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
        let Some(moving) = self.page_motion.as_ref().filter(|_| on_top) else {
            return page;
        };
        let now = motion::now(cx);
        if moving.tween.done(now) {
            self.page_motion = None;
            return page;
        }
        let progress = moving.tween.frame(window, cx);
        let placed = moving
            .slide
            .at(progress, moving.tween.time(now), moving.pop);
        let leaving = moving.leaving.clone().into_any_element();
        let (top, under) = match moving.pop {
            true => (leaving, page),
            false => (page, leaving),
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

    /// Whether a page change is moving.
    #[cfg(test)]
    pub(super) fn page_moving(&self) -> bool {
        self.page_motion.is_some()
    }
}
