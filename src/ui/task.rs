//! The shell's own cooperative work: a `Task` is a stream of messages the
//! reducer answers, a `Subscription` a persistent stream identified by its
//! recipe, not recreated per frame.
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
    pub fn batch(tasks: impl IntoIterator<Item = Self>) -> Self {
        let streams: Vec<_> = tasks.into_iter().filter_map(|task| task.0).collect();
        if streams.is_empty() {
            return Self::none();
        }
        Self::stream(futures::stream::select_all(streams))
    }
    pub fn map<U: 'static>(self, map: impl FnMut(T) -> U + 'static) -> Task<U> {
        Task(self.0.map(|stream| stream.map(map).boxed_local()))
    }
    pub fn then<U: 'static>(self, mut next: impl FnMut(T) -> Task<U> + 'static) -> Task<U> {
        Task(self.0.map(|stream| {
            stream
                .flat_map(move |value| next(value).into_stream())
                .boxed_local()
        }))
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
    pub fn abortable(self) -> (Self, Handle) {
        let (abort, registration) = futures::future::AbortHandle::new_pair();
        let task = Self::stream(futures::stream::Abortable::new(
            self.into_stream(),
            registration,
        ));
        (task, Handle { abort, guard: None })
    }
}

impl<T: 'static> Task<Option<T>> {
    pub fn and_then<U: 'static>(self, mut next: impl FnMut(T) -> Task<U> + 'static) -> Task<U> {
        self.then(move |value| value.map_or_else(Task::none, &mut next))
    }
}

pub struct Handle {
    abort: futures::future::AbortHandle,
    guard: Option<futures::future::AbortHandle>,
}
impl Handle {
    pub fn abort_on_drop(mut self) -> Self {
        self.guard = Some(self.abort.clone());
        self
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(abort) = &self.guard {
            abort.abort();
        }
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
