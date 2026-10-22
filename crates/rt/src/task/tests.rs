//! Each half's tasks, told the same story: one ends, one aborted is lost and drops what it held, a dropped one runs on,
//! a blocking job answers, a worker runs its task. Natively under `#[test]`, in a browser under wasm-bindgen-test.

use super::*;
use futures::channel::oneshot;

async fn a_task_ends_and_one_stopped_is_lost() {
    let mut ended = spawn(async {});
    assert_eq!((&mut ended).await, Ok(()));
    assert!(ended.is_finished());
    let mut never = spawn(std::future::pending());
    assert!(!never.is_finished());
    never.stopper().abort();
    assert_eq!((&mut never).await, Err(Lost));
    assert!(never.is_finished());
}

async fn an_aborted_task_drops_what_it_held() {
    let (held, dropped) = oneshot::channel::<()>();
    let mut task = spawn(async move {
        let _held = held;
        std::future::pending::<()>().await
    });
    task.abort();
    assert_eq!((&mut task).await, Err(Lost));
    assert_eq!(dropped.await, Err(oneshot::Canceled));
}

async fn a_dropped_task_runs_on() {
    let (go, gone) = oneshot::channel::<()>();
    let (tell, told) = oneshot::channel();
    drop(spawn(async move {
        let _ = gone.await;
        let _ = tell.send(7);
    }));
    let _ = go.send(());
    assert_eq!(told.await, Ok(7));
}

async fn a_blocking_job_answers() {
    let (tell, told) = oneshot::channel();
    drop(spawn(async move {
        let _ = tell.send(blocking(|| 7).await);
    }));
    assert_eq!(told.await, Ok(Ok(7)));
}

async fn a_worker_runs_its_task() -> Option<String> {
    let (tell, told) = oneshot::channel();
    worker("rt-test-worker", async move {
        let _ = tell.send(std::thread::current().name().map(str::to_owned));
    })
    .expect("a worker");
    told.await.expect("the worker ran")
}

#[cfg(not(target_family = "wasm"))]
mod native {
    use futures::executor::block_on;

    #[test]
    fn a_task_ends_and_one_stopped_is_lost() {
        block_on(super::a_task_ends_and_one_stopped_is_lost());
    }

    #[test]
    fn an_aborted_task_drops_what_it_held() {
        block_on(super::an_aborted_task_drops_what_it_held());
    }

    #[test]
    fn a_dropped_task_runs_on() {
        block_on(super::a_dropped_task_runs_on());
    }

    #[test]
    fn a_blocking_job_answers() {
        block_on(super::a_blocking_job_answers());
    }

    #[test]
    fn a_blocking_job_answers_a_thread_with_no_runtime() {
        let answered = std::thread::spawn(|| block_on(super::blocking(|| 7)))
            .join()
            .expect("the thread ran");
        assert_eq!(answered, Ok(7));
    }

    #[test]
    fn a_worker_runs_its_task_on_a_thread_of_its_name() {
        let named = block_on(super::a_worker_runs_its_task());
        assert_eq!(named.as_deref(), Some("rt-test-worker"));
    }

    #[test]
    fn every_task_runs_on_the_views_kernel_thread() {
        let mut task = super::spawn(async {
            assert_eq!(std::thread::current().name(), Some("views-kernel"));
        });
        assert_eq!(block_on(&mut task), Ok(()));
    }
}

#[cfg(target_family = "wasm")]
mod web {
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn a_task_ends_and_one_stopped_is_lost() {
        super::a_task_ends_and_one_stopped_is_lost().await;
    }

    #[wasm_bindgen_test]
    async fn an_aborted_task_drops_what_it_held() {
        super::an_aborted_task_drops_what_it_held().await;
    }

    #[wasm_bindgen_test]
    async fn a_dropped_task_runs_on() {
        super::a_dropped_task_runs_on().await;
    }

    #[wasm_bindgen_test]
    async fn a_blocking_job_answers() {
        super::a_blocking_job_answers().await;
    }

    #[wasm_bindgen_test]
    async fn a_worker_runs_its_task_on_the_page() {
        super::a_worker_runs_its_task().await;
    }
}
