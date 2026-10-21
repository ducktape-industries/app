//! Where a task runs where targets differ: the I/O loop (`spawn`), a thread that may hold a CPU (`worker`), an OS call (`blocking`).
//! Natively the `views-kernel` thread's tokio runtime, a named thread blocking on it, and tokio's blocking pool;
//! in a browser each is a task on the page's one thread.

use crate::MaybeSend;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lost;

impl std::fmt::Display for Lost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the task ended before it answered")
    }
}

impl std::error::Error for Lost {}

#[cfg(not(target_family = "wasm"))]
pub fn handle() -> tokio::runtime::Handle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let (send, recv) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("views-kernel".into())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("the views kernel runtime");
                    send.send(runtime.handle().clone())
                        .expect("the kernel handle is taken");
                    runtime.block_on(std::future::pending::<()>());
                })
                .expect("the views kernel thread");
            recv.recv().expect("the kernel runtime came up")
        })
        .clone()
}

#[cfg(not(target_family = "wasm"))]
pub struct Task(tokio::task::JoinHandle<()>);

#[cfg(not(target_family = "wasm"))]
#[derive(Clone)]
pub struct Stopper(tokio::task::AbortHandle);

#[cfg(not(target_family = "wasm"))]
pub fn spawn(task: impl Future<Output = ()> + MaybeSend + 'static) -> Task {
    Task(handle().spawn(task))
}

#[cfg(not(target_family = "wasm"))]
impl Task {
    pub fn abort(&self) {
        self.0.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }

    pub fn stopper(&self) -> Stopper {
        Stopper(self.0.abort_handle())
    }
}

#[cfg(not(target_family = "wasm"))]
impl Stopper {
    pub fn abort(&self) {
        self.0.abort();
    }
}

#[cfg(not(target_family = "wasm"))]
impl Future for Task {
    type Output = Result<(), Lost>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|ended| ended.map_err(|_| Lost))
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn worker(
    name: &str,
    task: impl Future<Output = ()> + MaybeSend + 'static,
) -> std::io::Result<()> {
    let runtime = handle();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || runtime.block_on(task))
        .map(drop)
}

#[cfg(not(target_family = "wasm"))]
pub fn blocking<T: MaybeSend + 'static>(
    job: impl FnOnce() -> T + MaybeSend + 'static,
) -> impl Future<Output = Result<T, Lost>> {
    let ran = handle().spawn_blocking(job);
    async move { ran.await.map_err(|_| Lost) }
}

#[cfg(target_family = "wasm")]
pub struct Task {
    stopper: Stopper,
    ended: futures::channel::oneshot::Receiver<()>,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(target_family = "wasm")]
#[derive(Clone)]
pub struct Stopper(futures::future::AbortHandle);

#[cfg(target_family = "wasm")]
pub fn spawn(task: impl Future<Output = ()> + MaybeSend + 'static) -> Task {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (stop, registration) = futures::future::AbortHandle::new_pair();
    let (tell, ended) = futures::channel::oneshot::channel();
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let marked = finished.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let ran = futures::future::Abortable::new(task, registration).await;
        marked.store(true, Ordering::Release);
        if ran.is_ok() {
            let _ = tell.send(());
        }
    });
    Task {
        stopper: Stopper(stop),
        ended,
        finished,
    }
}

#[cfg(target_family = "wasm")]
impl Task {
    pub fn abort(&self) {
        self.stopper.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }
}

#[cfg(target_family = "wasm")]
impl Stopper {
    pub fn abort(&self) {
        self.0.abort();
    }
}

#[cfg(target_family = "wasm")]
impl Future for Task {
    type Output = Result<(), Lost>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.ended)
            .poll(cx)
            .map(|ended| ended.map_err(|_| Lost))
    }
}

#[cfg(target_family = "wasm")]
pub fn worker(
    name: &str,
    task: impl Future<Output = ()> + MaybeSend + 'static,
) -> std::io::Result<()> {
    let _ = name;
    wasm_bindgen_futures::spawn_local(task);
    Ok(())
}

#[cfg(target_family = "wasm")]
pub fn blocking<T: MaybeSend + 'static>(
    job: impl FnOnce() -> T + MaybeSend + 'static,
) -> impl Future<Output = Result<T, Lost>> {
    let ran = job();
    async move { Ok(ran) }
}

#[cfg(test)]
mod tests;
