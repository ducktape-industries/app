//! Time where targets differ: tokio's timers natively, the page's `setTimeout` in a browser.

use futures::future::Either;
use std::future::Future;
use std::time::Duration;

#[cfg(not(target_family = "wasm"))]
pub use std::time::Instant;
#[cfg(target_family = "wasm")]
pub use web_time::Instant;

pub async fn sleep(duration: Duration) {
    #[cfg(not(target_family = "wasm"))]
    tokio::time::sleep(duration).await;
    #[cfg(target_family = "wasm")]
    gloo_timers::future::sleep(duration).await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Elapsed;

impl std::fmt::Display for Elapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the deadline passed")
    }
}

impl std::error::Error for Elapsed {}

pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, Elapsed> {
    let future = std::pin::pin!(future);
    let deadline = std::pin::pin!(sleep(duration));
    match futures::future::select(future, deadline).await {
        Either::Left((output, _)) => Ok(output),
        Either::Right(_) => Err(Elapsed),
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_future_past_its_deadline_is_elapsed_and_one_before_it_answers() {
        let never = std::future::pending::<()>();
        assert_eq!(timeout(Duration::from_secs(30), never).await, Err(Elapsed));
        let at_once = async { 7 };
        assert_eq!(timeout(Duration::from_secs(30), at_once).await, Ok(7));
    }
}
