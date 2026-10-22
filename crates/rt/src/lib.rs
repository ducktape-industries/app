//! The process's runtime where targets differ: the futures and streams a client hands on, time, where a task runs,
//! and what the process knows of itself. Natively `Send`, tokio's and libc's; in a browser the page's, on its one thread.

pub mod process;
pub mod task;
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
