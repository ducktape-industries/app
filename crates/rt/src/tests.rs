//! A process-wide value is one value wherever it is read: natively from any thread, in a browser on the page.

crate::process_wide!(fn counted() -> u64 = 0;);

fn count() -> u64 {
    counted(|count| {
        *count += 1;
        *count
    })
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn a_process_wide_value_is_one_across_threads() {
    let first = count();
    let second = std::thread::spawn(count).join().expect("the thread ran");
    assert_eq!(second, first + 1);
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen_test::wasm_bindgen_test]
fn the_pages_process_id_is_one_number_for_the_page() {
    assert_eq!(crate::process::id(), crate::process::id());
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen_test::wasm_bindgen_test]
fn a_process_wide_value_is_one_across_reads() {
    let first = count();
    assert_eq!(count(), first + 1);
}
