//! How the pass's time grows with a module's imports (one function added per
//! imported function). Its own binary and the best of a few runs: a timing
//! ratio taken beside the equivalence suite's threads, on a loaded machine,
//! measured the load (the re-check of #547, n1).
#![allow(clippy::unwrap_used, reason = "a test fails loudly")]

use std::time::{Duration, Instant};

fn module(imports: usize) -> Vec<u8> {
    let imports: String = (0..imports)
        .map(|i| format!("(import \"m\" \"f{i}\" (func))"))
        .collect();
    wat::parse_str(format!("(module {imports} (func (export \"f\")))")).unwrap()
}

/// The fastest of three `accepts` + `meter` runs over `code`.
fn best_of_three(code: &[u8]) -> Duration {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            view_meter::accepts(code).unwrap();
            view_meter::meter(code, 64 << 20).unwrap();
            start.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn many_imports_meter_in_linear_time() {
    let (small, large) = (module(10_000), module(40_000));
    let (small, large) = (best_of_three(&small), best_of_three(&large));
    // four times the imports: about four times the time, not sixteen
    assert!(
        large < small * 8,
        "10k imports {small:?}, 40k imports {large:?}"
    );
}
