use super::*;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const PAGE: usize = 65_536;

fn view(more: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
        (import "ducktape_view" "panicked" (func $panicked (param i32 i32)))
        (memory (export "memory") 1)
        (data (i32.const 1024) "boom\0amore")
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        {more})"#
    ))
    .expect("parses")
}

const TICKS: &str = r#"(func (export "init"))
    (func (export "tick") (param i32 i32) (result i64)
        (i64.or (i64.shl (i64.extend_i32_u (local.get 0)) (i64.const 32)) (i64.extend_i32_u (local.get 1))))"#;

async fn seat(runner: &WebRunner, hash: [u8; 32]) -> (Landed, async_channel::Receiver<Landed>) {
    let (land, landing) = Landing::channel();
    runner.instantiate(hash, SeatId("web-test", 0), land);
    let landed = landing.recv().await.expect("lands");
    (landed, landing)
}

async fn seated(
    runner: &WebRunner,
    hash: [u8; 32],
) -> (Box<dyn ViewInstance>, async_channel::Receiver<Landed>) {
    match seat(runner, hash).await {
        (Landed::Seated(instance), landing) => (instance, landing),
        (other, _) => panic!("not seated: {other:?}"),
    }
}

fn now(landing: &async_channel::Receiver<Landed>) -> Landed {
    landing.try_recv().expect("a call lands before it returns")
}

#[wasm_bindgen_test]
async fn a_tick_hands_its_events_over_and_lands_the_frame() {
    let runner = WebRunner::new();
    let hash = [1; 32];
    let held = runner
        .prepare(hash, Code::of(&view(TICKS)), 4 * PAGE)
        .expect("prepares");
    assert_eq!(held.source, CodeSource::Cold);
    assert_eq!(
        runner
            .prepare(hash, Code::of(&view(TICKS)), 4 * PAGE)
            .expect("prepares")
            .source,
        CodeSource::Memory
    );
    let (mut instance, landing) = seated(&runner, hash).await;
    instance.begin_init();
    assert!(matches!(now(&landing), Landed::Inited));
    instance.begin_tick(b"events".to_vec());
    let frame = now(&landing);
    assert!(
        matches!(&frame, Landed::Frame(bytes) if bytes == b"events"),
        "{frame:?}"
    );
    assert!(instance.fuel_used() > 0);
    assert_eq!(instance.memory_bytes(), PAGE);
}

#[wasm_bindgen_test]
async fn a_grow_past_the_view_memory_setting_is_memory() {
    let runner = WebRunner::new();
    let hash = [2; 32];
    let code = view(
        r#"(func (export "init") (drop (memory.grow (i32.const 8))))
        (func (export "tick") (param i32 i32) (result i64) i64.const 0)"#,
    );
    let _held = runner
        .prepare(hash, Code::of(&code), 4 * PAGE)
        .expect("prepares");
    let (mut instance, landing) = seated(&runner, hash).await;
    instance.begin_init();
    let landed = now(&landing);
    assert!(matches!(landed, Landed::Fault(Fault::Memory)), "{landed:?}");
}

#[wasm_bindgen_test]
async fn a_panic_hook_hands_its_first_line_over() {
    let runner = WebRunner::new();
    let hash = [3; 32];
    let code = view(
        r#"(func (export "init") (call $panicked (i32.const 1024) (i32.const 9)) unreachable)
        (func (export "tick") (param i32 i32) (result i64) unreachable)"#,
    );
    let _held = runner
        .prepare(hash, Code::of(&code), 4 * PAGE)
        .expect("prepares");
    let (mut instance, landing) = seated(&runner, hash).await;
    instance.begin_init();
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::Fault(Fault::Panicked(words)) if words == "boom"),
        "{landed:?}"
    );
    instance.begin_tick(Vec::new());
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::Fault(Fault::Trap(_))),
        "a trap with no panic before it: {landed:?}"
    );
}

#[wasm_bindgen_test]
async fn a_view_missing_an_export_is_a_fault() {
    let runner = WebRunner::new();
    let hash = [4; 32];
    let _held = runner
        .prepare(hash, Code::of(&view(r#"(func (export "init"))"#)), 4 * PAGE)
        .expect("prepares");
    let (landed, _) = seat(&runner, hash).await;
    assert!(
        matches!(&landed, Landed::Fault(Fault::Trap(words)) if words == "the view exports no tick"),
        "{landed:?}"
    );
}

#[wasm_bindgen_test]
async fn a_module_without_snapshot_and_restore_seats_and_traps_when_asked() {
    let runner = WebRunner::new();
    let hash = [5; 32];
    let _held = runner
        .prepare(hash, Code::of(&view(TICKS)), 4 * PAGE)
        .expect("prepares");
    let (mut instance, landing) = seated(&runner, hash).await;
    instance.begin_snapshot();
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::Fault(Fault::Trap(words)) if words == "the view exports no snapshot"),
        "{landed:?}"
    );
    instance.begin_restore(b"state".to_vec());
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::Fault(Fault::Trap(words)) if words == "the view exports no restore"),
        "{landed:?}"
    );
}

#[wasm_bindgen_test]
async fn snapshot_and_restore_answer_a_result() {
    let runner = WebRunner::new();
    let hash = [6; 32];
    let code = view(
        r#"(data (i32.const 2048) "\00state\01busy")
        (func (export "init"))
        (func (export "tick") (param i32 i32) (result i64) i64.const 0)
        (func (export "snapshot") (result i64) i64.const 0x80000000006)
        (func (export "restore") (param i32 i32) (result i64) i64.const 0x80600000005)"#,
    );
    let _held = runner
        .prepare(hash, Code::of(&code), 4 * PAGE)
        .expect("prepares");
    let (mut instance, landing) = seated(&runner, hash).await;
    instance.begin_snapshot();
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::Snapshot(state) if state == b"state"),
        "{landed:?}"
    );
    instance.begin_restore(b"state".to_vec());
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::RestoreRefused(words) if words == "busy"),
        "{landed:?}"
    );
}

#[wasm_bindgen_test]
async fn an_unknown_import_traps_when_called() {
    let runner = WebRunner::new();
    let hash = [7; 32];
    let code = wat::parse_str(
        r#"(module
        (import "env" "elsewhere" (func $elsewhere))
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 64)
        (func (export "init") call $elsewhere)
        (func (export "tick") (param i32 i32) (result i64) i64.const 0))"#,
    )
    .expect("parses");
    let _held = runner
        .prepare(hash, Code::of(&code), 4 * PAGE)
        .expect("prepares");
    let (mut instance, landing) = seated(&runner, hash).await;
    instance.begin_init();
    let landed = now(&landing);
    assert!(
        matches!(&landed, Landed::Fault(Fault::Trap(words)) if words.contains("env::elsewhere")),
        "{landed:?}"
    );
}

#[wasm_bindgen_test]
async fn code_nothing_holds_is_freed() {
    let runner = WebRunner::new();
    let hash = [8; 32];
    let held = runner
        .prepare(hash, Code::of(&view(TICKS)), 4 * PAGE)
        .expect("prepares");
    let (instance, _) = seated(&runner, hash).await;
    drop(held);
    assert!(
        matches!(seat(&runner, hash).await.0, Landed::Seated(_)),
        "a running instance holds its code"
    );
    drop(instance);
    let (landed, _) = seat(&runner, hash).await;
    assert!(
        matches!(landed, Landed::Fault(Fault::Runner(_))),
        "{landed:?}"
    );
    assert_eq!(
        runner
            .prepare(hash, Code::of(&view(TICKS)), 4 * PAGE)
            .expect("prepares")
            .source,
        CodeSource::Cold
    );
}

#[wasm_bindgen_test]
fn the_runner_refuses_what_the_meter_refuses() {
    let runner = WebRunner::new();
    let started = wat::parse_str(r#"(module (memory (export "memory") 1) (func $go) (start $go))"#)
        .expect("parses");
    assert!(matches!(
        runner.prepare([9; 32], Code::of(&started), 4 * PAGE),
        Err(Failure::Unsupported(_))
    ));
    let big = wat::parse_str(r#"(module (memory (export "memory") 8))"#).expect("parses");
    assert!(matches!(
        runner.prepare([10; 32], Code::of(&big), 4 * PAGE),
        Err(Failure::Runner(_))
    ));
}

const HI: &str = r#"(module
    (memory (export "memory") 1)
    (data (i32.const 1024) "\02\00\00\00hi\00\00\00\00")
    (func (export "alloc") (param i32) (result i32) i32.const 0)
    (func (export "describe") (param i32 i32) (result i64) i64.const 0x4000000000a))"#;

const LOOPS: &str = r#"(module
    (memory (export "memory") 1)
    (func (export "alloc") (param i32) (result i32) i32.const 0)
    (func (export "describe") (param i32 i32) (result i64) (loop (br 0)) i64.const 0))"#;

const IMPORTS: &str = r#"(module
    (import "env" "f" (func))
    (memory (export "memory") 1)
    (data (i32.const 1024) "\02\00\00\00hi\00\00\00\00")
    (func (export "alloc") (param i32) (result i32) i32.const 0)
    (func (export "describe") (param i32 i32) (result i64) i64.const 0x4000000000a))"#;

#[wasm_bindgen_test]
async fn an_op_reads_as_its_describe_module_says_and_a_hostile_one_reads_as_nothing() {
    let runner = WebRunner::new();
    let described = runner
        .describe(
            [11; 32],
            Code::of(&wat::parse_str(HI).expect("parses")),
            vec![1, 2],
        )
        .await;
    assert_eq!(
        described.map(|described| described.title).as_deref(),
        Some("hi")
    );
    for (hash, module) in [([12; 32], LOOPS), ([13; 32], IMPORTS)] {
        let module = wat::parse_str(module).expect("parses");
        assert!(
            runner
                .describe(hash, Code::of(&module), vec![1])
                .await
                .is_none(),
            "{hash:?}"
        );
    }
    assert!(
        runner
            .describe([14; 32], Code::of(b"not wasm"), vec![1])
            .await
            .is_none()
    );
}
