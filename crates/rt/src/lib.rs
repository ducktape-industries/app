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

#[macro_export]
macro_rules! process_wide {
    ($vis:vis fn $name:ident() -> $held:ty = $init:expr;) => {
        #[cfg(not(target_family = "wasm"))]
        $vis fn $name<R>(with: impl FnOnce(&mut $held) -> R) -> R {
            static HELD: std::sync::Mutex<$held> = std::sync::Mutex::new($init);
            let mut held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            with(&mut held)
        }

        #[cfg(target_family = "wasm")]
        $vis fn $name<R>(with: impl FnOnce(&mut $held) -> R) -> R {
            std::thread_local! {
                static HELD: std::cell::RefCell<$held> = const { std::cell::RefCell::new($init) };
            }
            HELD.with_borrow_mut(with)
        }
    };
}

#[cfg(test)]
mod tests;
