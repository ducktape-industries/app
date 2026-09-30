//! The shell's own cooperative work: a `Task` is a stream of messages the
//! reducer answers, a `Subscription` a persistent stream identified by its
//! recipe, not recreated per frame. (An entity's work is a `gpui::Task`:
//! `entities::spawn_on_runtime`.)
use futures::{Stream, StreamExt};
use std::any::TypeId;
use std::hash::{Hash, Hasher};

pub type BoxStream<T> = futures::stream::LocalBoxStream<'static, T>;

pub struct Task<T>(Option<BoxStream<T>>);

impl<T: 'static> Task<T> {
    pub fn none() -> Self {
        Self(None)
    }
    pub fn stream(stream: impl Stream<Item = T> + 'static) -> Self {
        Self(Some(stream.boxed_local()))
    }
    pub fn into_stream(self) -> BoxStream<T> {
        self.0
            .unwrap_or_else(|| futures::stream::empty().boxed_local())
    }
}

pub struct Recipe<T> {
    pub key: u64,
    pub start: Box<dyn FnOnce() -> BoxStream<T>>,
}
pub struct Subscription<T> {
    recipes: Vec<Recipe<T>>,
}

fn fingerprint(value: impl Hash) -> u64 {
    let mut hash = std::hash::DefaultHasher::new();
    value.hash(&mut hash);
    hash.finish()
}

impl<T: 'static> Subscription<T> {
    pub fn into_recipes(self) -> Vec<Recipe<T>> {
        self.recipes
    }
    pub fn run<S: Stream<Item = T> + 'static>(make: fn() -> S) -> Self {
        Self {
            recipes: vec![Recipe {
                key: fingerprint((TypeId::of::<S>(), make as usize)),
                start: Box::new(move || make().boxed_local()),
            }],
        }
    }
}
