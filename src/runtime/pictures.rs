//! A seat's guest pictures. A frame brings a picture's bytes once and
//! later frames name it by hash, so the bytes outlive the frame and the
//! native tree that drew them. The store holds each picture's bytes once
//! (an `Arc`), takes them out of the tree it receives, and hands every tree
//! the seat draws the same bytes to resolve hashes through
//! (`render::PictureBytes`).
//!
//! The store is bounded. Past [`MAX_PICTURE_BYTES`] or [`MAX_PICTURES`] it
//! keeps what the current tree names, in tree order, while it fits, and
//! evicts the rest. The guest names a picture by hash alone once it has
//! sent it, so an evicted picture could never come back unasked: an
//! eviction owes the guest `Event::Resync`, which makes it forget what it
//! sent and send every picture it draws again.

use crate::render::PictureBytes;
use std::sync::Arc;
use view_wire as wire;

/// The raw picture bytes one seat holds: a view at the budget costs this
/// much host memory, whatever it draws (P16's measurement is in its report).
pub(super) const MAX_PICTURE_BYTES: u64 = 64 << 20;
/// The pictures one seat holds, however small: the guest SDK forgets what
/// it sent at the same count.
pub(super) const MAX_PICTURES: usize = 4096;

#[derive(Default)]
pub(crate) struct Pictures {
    held: Arc<PictureBytes>,
    bytes: u64,
    /// The first eviction is logged; later ones are the same story.
    evicted: bool,
}

impl Pictures {
    /// The bytes every tree the seat draws resolves its hashes through.
    pub(crate) fn held(&self) -> Arc<PictureBytes> {
        self.held.clone()
    }

    /// Every byte held.
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Moves every picture's bytes out of `root` and the tooltip trees it
    /// holds, leaving the hashes: the tree as the guest remembers it. A hash
    /// already held keeps its first bytes. Then holds the budget against
    /// `root`; `true` when it evicted, and the guest is owed a `Resync`.
    pub(super) fn adopt(&mut self, root: &mut wire::Node, module: &str) -> bool {
        let mut raster = Vec::new();
        let mut vector = Vec::new();
        each_picture(root, &mut |picture| match picture {
            Picture::Raster(hash, data) => raster.extend(data.take().map(|data| (hash, data))),
            Picture::Vector(hash, bytes) => vector.extend(bytes.take().map(|bytes| (hash, bytes))),
        });
        let fresh = PictureBytes {
            raster: (raster.into_iter())
                .map(|(hash, data)| (hash, Arc::new(data)))
                .collect(),
            vector: (vector.into_iter())
                .map(|(hash, bytes)| (hash, Arc::from(bytes)))
                .collect(),
        };
        self.join(fresh);
        self.hold(root, module)
    }

    /// The drawn view's store, carried to its replacement across a hot
    /// swap: `fresh`, the replacement's own, joins it (the drawn view's
    /// bytes win a shared hash), and the budget holds against `root`, the
    /// replacement's first tree. `true` when it evicted.
    pub(super) fn carry(&mut self, fresh: Pictures, root: &mut wire::Node, module: &str) -> bool {
        self.join(Arc::unwrap_or_clone(fresh.held));
        self.hold(root, module)
    }

    /// Takes in the pictures of `fresh` whose hash is not held yet.
    fn join(&mut self, fresh: PictureBytes) {
        let held = &self.held;
        let raster: Vec<_> = (fresh.raster.into_iter())
            .filter(|(hash, _)| !held.raster.contains_key(hash))
            .collect();
        let vector: Vec<_> = (fresh.vector.into_iter())
            .filter(|(hash, _)| !held.vector.contains_key(hash))
            .collect();
        if raster.is_empty() && vector.is_empty() {
            return;
        }
        // the trees drawing the old map keep it until they are handed this one
        let held = Arc::make_mut(&mut self.held);
        for (hash, data) in raster {
            self.bytes += data.byte_len() as u64;
            held.raster.insert(hash, data);
        }
        for (hash, bytes) in vector {
            self.bytes += bytes.len() as u64;
            held.vector.insert(hash, bytes);
        }
    }

    /// Past the budget, keeps what `root` names, in tree order, while it
    /// fits, and evicts the rest. `true` when it evicted.
    fn hold(&mut self, root: &mut wire::Node, module: &str) -> bool {
        let count = self.held.raster.len() + self.held.vector.len();
        if self.bytes <= MAX_PICTURE_BYTES && count <= MAX_PICTURES {
            return false;
        }
        let held = &self.held;
        let mut kept = PictureBytes::default();
        let mut bytes = 0;
        each_picture(root, &mut |picture| {
            let count = kept.raster.len() + kept.vector.len();
            let fits = |len: usize| count < MAX_PICTURES && bytes + len as u64 <= MAX_PICTURE_BYTES;
            match picture {
                Picture::Raster(hash, _) => {
                    if let Some(data) = held.raster.get(&hash)
                        && !kept.raster.contains_key(&hash)
                        && fits(data.byte_len())
                    {
                        bytes += data.byte_len() as u64;
                        kept.raster.insert(hash, data.clone());
                    }
                }
                Picture::Vector(hash, _) => {
                    if let Some(data) = held.vector.get(&hash)
                        && !kept.vector.contains_key(&hash)
                        && fits(data.len())
                    {
                        bytes += data.len() as u64;
                        kept.vector.insert(hash, data.clone());
                    }
                }
            }
        });
        if !self.evicted {
            self.evicted = true;
            tracing::warn!(
                target: "ducktape::app",
                module,
                reason = "module_view_pictures_evicted",
                held = self.bytes,
                kept = bytes,
                budget = MAX_PICTURE_BYTES,
                "module view pictures past the seat's budget; evicted, view asked to resend"
            );
        }
        self.held = Arc::new(kept);
        self.bytes = bytes;
        true
    }
}

/// One picture node: its hash, and the bytes it brings.
enum Picture<'a> {
    Raster(u64, &'a mut Option<wire::ImageData>),
    Vector(u64, &'a mut Option<Vec<u8>>),
}

/// Every picture in `root` and in the tooltip trees its nodes hold, in tree
/// order (a node's tooltip after the node, before its children).
fn each_picture(root: &mut wire::Node, visit: &mut dyn FnMut(Picture<'_>)) {
    root.for_each_mut(&mut |node| {
        match node {
            wire::Node::Image { hash, data, .. } => visit(Picture::Raster(*hash, data)),
            wire::Node::Svg {
                source: wire::SvgSource::Data { hash, bytes },
                ..
            } => visit(Picture::Vector(*hash, bytes)),
            _ => {}
        }
        if let Some(content) = tooltip_content(node) {
            each_picture(content, &mut *visit);
        }
    });
}

/// The tree a node's tooltip holds, if it holds one.
fn tooltip_content(node: &mut wire::Node) -> Option<&mut wire::Node> {
    let content = match node {
        wire::Node::Container(view_wire::ContainerNode { interactivity, .. })
        | wire::Node::UniformList { interactivity, .. }
        | wire::Node::List { interactivity, .. }
        | wire::Node::ResizeHandle { interactivity, .. }
        | wire::Node::Image { interactivity, .. }
        | wire::Node::Svg { interactivity, .. } => &mut interactivity.tooltip.as_mut()?.content,
        wire::Node::RichText { tooltip, .. } => &mut tooltip.as_mut()?.content,
        _ => return None,
    };
    content.as_deref_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(hash: u64, bytes: Option<Vec<u8>>) -> wire::Node {
        wire::Node::Svg {
            id: None,
            source: wire::SvgSource::Data { hash, bytes },
            transformation: wire::SvgTransformation {
                scale: [1., 1.],
                translate: [0., 0.],
                rotate: 0.,
            },
            label: None,
            style: Default::default(),
            interactivity: Default::default(),
        }
    }

    fn held_vector(pictures: &Pictures, hash: u64) -> Option<Vec<u8>> {
        pictures
            .held()
            .vector
            .get(&hash)
            .map(|bytes| bytes.to_vec())
    }

    #[test]
    fn hidden_pictures_survive_remount_and_the_first_hash_value_wins() {
        let mut pictures = Pictures::default();
        pictures.adopt(&mut vector(7, Some(b"first".to_vec())), "test");
        pictures.adopt(&mut wire::Node::empty(), "test");
        let mut conflicting = vector(7, Some(b"conflicting".to_vec()));
        pictures.adopt(&mut conflicting, "test");
        assert_eq!(
            conflicting,
            vector(7, None),
            "the tree keeps the hash alone"
        );
        assert_eq!(held_vector(&pictures, 7).as_deref(), Some(&b"first"[..]));
        assert_eq!(pictures.bytes(), 5);
    }

    /// A tooltip's tree is drawn from the same store: its pictures are
    /// taken in, and named, like the tree's own.
    #[test]
    fn a_tooltips_pictures_are_held_and_named() {
        let mut root = wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: Default::default(),
            interactivity: wire::Interactivity {
                tooltip: Some(wire::Tooltip {
                    request: 1,
                    content: Some(Box::new(vector(9, Some(b"tip".to_vec())))),
                    hoverable: false,
                    delay_ms: 0,
                }),
                ..Default::default()
            },
            children: Vec::new(),
        });
        let mut pictures = Pictures::default();
        pictures.adopt(&mut root, "test");
        assert_eq!(held_vector(&pictures, 9).as_deref(), Some(&b"tip"[..]));
        let mut named = Vec::new();
        each_picture(&mut root, &mut |picture| match picture {
            Picture::Vector(hash, bytes) => named.push((hash, bytes.is_some())),
            Picture::Raster(..) => {}
        });
        assert_eq!(named, [(9, false)], "named by hash alone");
    }

    /// A hot swap carries the drawn view's pictures to its replacement:
    /// the replacement's own join them, and the drawn view's bytes win a
    /// hash both hold.
    #[test]
    fn a_swap_carries_the_drawn_views_pictures_and_the_first_bytes_win() {
        let mut old = Pictures::default();
        old.adopt(&mut vector(7, Some(b"first".to_vec())), "test");
        let mut fresh = Pictures::default();
        let mut first_tree = wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: Default::default(),
            interactivity: Default::default(),
            children: vec![
                vector(7, Some(b"conflicting".to_vec())),
                vector(9, Some(b"new".to_vec())),
            ],
        });
        fresh.adopt(&mut first_tree, "test");
        assert!(
            !old.carry(fresh, &mut first_tree, "test"),
            "under the budget"
        );
        assert_eq!(held_vector(&old, 7).as_deref(), Some(&b"first"[..]));
        assert_eq!(held_vector(&old, 9).as_deref(), Some(&b"new"[..]));
        assert_eq!(old.bytes(), 8);
    }

    #[test]
    fn host_image_resource_survives_a_patch_and_remount() {
        let mut pictures = Pictures::default();
        let mut image = wire::Node::Image {
            id: None,
            hash: 11,
            data: Some(wire::ImageData::Resource("image:7".into())),
            label: None,
            image_style: wire::ImageStyle {
                grayscale: false,
                object_fit: wire::ImageObjectFit::Contain,
            },
            loading: false,
            fallback: false,
            state_children: vec![],
            style: Default::default(),
            interactivity: Default::default(),
        };
        pictures.adopt(&mut image, "test");
        assert!(matches!(image, wire::Node::Image { data: None, .. }));
        pictures.adopt(&mut image, "test");
        assert!(matches!(
            pictures.held().raster.get(&11).map(|data| &**data),
            Some(wire::ImageData::Resource(key)) if key == "image:7"
        ));
    }
}
