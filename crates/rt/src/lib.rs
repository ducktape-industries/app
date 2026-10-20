//! The process's runtime where targets differ: the futures and streams a client hands on, and time.
//! Natively they are `Send` and tokio's; in a browser they live on the page's one thread.

pub mod time;

#[cfg(not(target_family = "wasm"))]
pub type Fut<'a, T> = futures::future::BoxFuture<'a, T>;
#[cfg(target_family = "wasm")]
pub type Fut<'a, T> = futures::future::LocalBoxFuture<'a, T>;

#[cfg(not(target_family = "wasm"))]
pub type BoxStream<'a, T> = futures::stream::BoxStream<'a, T>;
#[cfg(target_family = "wasm")]
pub type BoxStream<'a, T> = futures::stream::LocalBoxStream<'a, T>;

#[cfg(not(target_family = "wasm"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_family = "wasm"))]
impl<T: Send + ?Sized> MaybeSend for T {}

#[cfg(target_family = "wasm")]
pub trait MaybeSend {}
#[cfg(target_family = "wasm")]
impl<T: ?Sized> MaybeSend for T {}
