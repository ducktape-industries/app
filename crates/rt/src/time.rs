//! Time where targets differ: tokio's timers natively, the page's `setTimeout` in a browser.

use futures::future::Either;
use std::future::Future;
use std::time::Duration;

#[cfg(not(target_family = "wasm"))]
pub use std::time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_family = "wasm")]
pub use web_time::{Instant, SystemTime, UNIX_EPOCH};

pub async fn sleep(duration: Duration) {
    #[cfg(not(target_family = "wasm"))]
    tokio::time::sleep(duration).await;
    #[cfg(target_family = "wasm")]
    gloo_timers::future::sleep(duration).await;
}

pub async fn sleep_until(deadline: Instant) {
    #[cfg(not(target_family = "wasm"))]
    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
    #[cfg(target_family = "wasm")]
    sleep(deadline.saturating_duration_since(Instant::now())).await;
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

    #[tokio::test(start_paused = true)]
    async fn a_sleep_until_a_deadline_ends_there_and_not_before() {
        let deadline = Instant::now() + Duration::from_secs(30);
        let early = timeout(Duration::from_secs(29), sleep_until(deadline)).await;
        assert_eq!(early, Err(Elapsed));
        let late = timeout(Duration::from_secs(2), sleep_until(deadline)).await;
        assert_eq!(late, Ok(()));
    }
}
