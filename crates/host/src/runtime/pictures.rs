//! A seat's view pictures. A frame brings a picture's bytes once and
//! later frames name it by hash, so the bytes outlive the frame and the
//! native tree that drew them. The store holds each picture's bytes once
//! (an `Arc`), takes them out of the tree it receives, and hands every tree
//! the seat draws the same bytes to resolve hashes through
//! (`render::PictureBytes`).
//!
//! The store is bounded by the device's picture memory setting
//! (`backend::memory_bounds`), each picture charged its bytes and its entry.
//! Past it the store keeps what the current tree names, in tree order, while
//! it fits, and evicts the rest. The view names a picture by hash alone once it has
//! sent it, so an evicted picture could never come back unasked: an
//! eviction owes the view `Event::Resync`, which makes it forget what it
//! sent and send every picture it draws again.

use std::collections::HashMap;
use std::sync::Arc;
use view_wire as wire;

/// A seat's picture bytes by content hash, each held once: the seat's
/// store (this module) owns and evicts them, and hands every tree it draws
/// this map to resolve the hashes it names (`render::pictures`).
#[derive(Clone, Default)]
pub struct PictureBytes {
    pub raster: HashMap<u64, Arc<wire::ImageData>>,
    pub vector: HashMap<u64, Arc<[u8]>>,
}

const ENTRY_BYTES: u64 = std::mem::size_of::<(u64, Arc<wire::ImageData>)>() as u64;

pub(super) fn charge(bytes: u64, pictures: usize) -> u64 {
    bytes.saturating_add((pictures as u64).saturating_mul(ENTRY_BYTES))
}

#[derive(Default)]
pub struct Pictures {
    held: Arc<PictureBytes>,
    bytes: u64,
    evicted: bool,
}

impl Pictures {
    /// The bytes every tree the seat draws resolves its hashes through.
    pub fn held(&self) -> Arc<PictureBytes> {
        self.held.clone()
    }

    /// Every byte held.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(super) fn adopt(&mut self, root: &mut wire::Node, program: &str) -> bool {
        self.join(bytes_of(root));
        self.hold(root, program)
    }

    pub(super) fn take(
        &mut self,
        content: &mut wire::Node,
        root: &mut wire::Node,
        program: &str,
    ) -> bool {
        self.join(bytes_of(content));
        self.hold(root, program)
    }

    pub(super) fn carry(&mut self, fresh: Pictures, root: &mut wire::Node, program: &str) -> bool {
        self.join(Arc::unwrap_or_clone(fresh.held));
        self.hold(root, program)
    }

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

    fn hold(&mut self, root: &mut wire::Node, program: &str) -> bool {
        let budget = crate::backend::memory_bounds().picture_bytes;
        let count = self.held.raster.len() + self.held.vector.len();
        let within = charge(self.bytes, count) <= budget;
        if within {
            return false;
        }
        let held = &self.held;
        let mut kept = PictureBytes::default();
        let mut bytes = 0;
        each_picture(root, &mut |picture| {
            let count = kept.raster.len() + kept.vector.len();
            let fits = |len: usize| charge(bytes + len as u64, count + 1) <= budget;
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
                program,
                reason = "program_view_pictures_evicted",
                held = self.bytes,
                kept = bytes,
                budget,
                "program view pictures past the seat's budget; evicted, view asked to resend"
            );
        }
        self.held = Arc::new(kept);
        self.bytes = bytes;
        true
    }
}

enum Picture<'a> {
    Raster(u64, &'a mut Option<wire::ImageData>),
    Vector(u64, &'a mut Option<Vec<u8>>),
}

fn each_picture(root: &mut wire::Node, visit: &mut dyn FnMut(Picture<'_>)) {
    root.for_each_mut(&mut |node| match node {
        wire::Node::Image { hash, data, .. } => visit(Picture::Raster(*hash, data)),
        wire::Node::Svg {
            source: wire::SvgSource::Data { hash, bytes },
            ..
        } => visit(Picture::Vector(*hash, bytes)),
        _ => {}
    });
}

fn bytes_of(root: &mut wire::Node) -> PictureBytes {
    let mut raster = Vec::new();
    let mut vector = Vec::new();
    each_picture(root, &mut |picture| match picture {
        Picture::Raster(hash, data) => raster.extend(data.take().map(|data| (hash, data))),
        Picture::Vector(hash, bytes) => vector.extend(bytes.take().map(|bytes| (hash, bytes))),
    });
    PictureBytes {
        raster: (raster.into_iter())
            .map(|(hash, data)| (hash, Arc::new(data)))
            .collect(),
        vector: (vector.into_iter())
            .map(|(hash, bytes)| (hash, Arc::from(bytes)))
            .collect(),
    }
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
            style: crate::render::plain_style(),
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

    #[test]
    fn a_tooltips_pictures_are_held_and_named() {
        let mut root = wire::Node::empty();
        let mut content = vector(9, Some(b"tip".to_vec()));
        let mut pictures = Pictures::default();
        pictures.take(&mut content, &mut root, "test");
        assert_eq!(held_vector(&pictures, 9).as_deref(), Some(&b"tip"[..]));
        let mut named = Vec::new();
        each_picture(&mut content, &mut |picture| match picture {
            Picture::Vector(hash, bytes) => named.push((hash, bytes.is_some())),
            Picture::Raster(..) => {}
        });
        assert_eq!(named, [(9, false)], "named by hash alone");
    }

    #[test]
    fn a_swap_carries_the_drawn_views_pictures_and_the_first_bytes_win() {
        let mut old = Pictures::default();
        old.adopt(&mut vector(7, Some(b"first".to_vec())), "test");
        let mut fresh = Pictures::default();
        let mut first_tree = wire::Node::Container(view_wire::ContainerNode {
            id: None,
            style: crate::render::plain_style(),
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
            style: crate::render::plain_style(),
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
