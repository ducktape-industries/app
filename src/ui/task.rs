//! The shell's own cooperative work: a `Task` is a stream of messages the
//! reducer answers, a `Subscription` a persistent stream identified by its
//! recipe, not recreated per frame. (An entity's work is a `gpui::Task`:
//! `entities::spawn_on_runtime`.)
use futures::{Stream, StreamExt};
use std::any::TypeId;
use std::future::Future;
use std::hash::{Hash, Hasher};

pub type BoxStream<T> = futures::stream::LocalBoxStream<'static, T>;

pub struct Task<T>(Option<BoxStream<T>>);

impl<T: 'static> Task<T> {
    pub fn none() -> Self {
        Self(None)
    }
    pub fn done(value: T) -> Self {
        Self::future(std::future::ready(value))
    }
    pub fn future(future: impl Future<Output = T> + 'static) -> Self {
        Self::stream(futures::stream::once(future))
    }
    pub fn stream(stream: impl Stream<Item = T> + 'static) -> Self {
        Self(Some(stream.boxed_local()))
    }
    pub fn map<U: 'static>(self, map: impl FnMut(T) -> U + 'static) -> Task<U> {
        Task(self.0.map(|stream| stream.map(map).boxed_local()))
    }
    pub fn discard<U: 'static>(self) -> Task<U> {
        Task(self.0.map(|stream| {
            stream
                .filter_map(|_| std::future::ready(None))
                .boxed_local()
        }))
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
    pub fn none() -> Self {
        Self {
            recipes: Vec::new(),
        }
    }
    pub fn run<S: Stream<Item = T> + 'static>(make: fn() -> S) -> Self {
        Self {
            recipes: vec![Recipe {
                key: fingerprint((TypeId::of::<S>(), make as usize)),
                start: Box::new(move || make().boxed_local()),
            }],
        }
    }
    pub fn batch(subscriptions: impl IntoIterator<Item = Self>) -> Self {
        let mut result = Self::none();
        for mut subscription in subscriptions {
            result.recipes.append(&mut subscription.recipes);
        }
        result
    }
}
