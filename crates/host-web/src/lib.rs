//! The browser's view runner: view-runner's contract over the page's own WebAssembly, reached from Rust through js-sys.
//! `prepare` meters the code (the page counts no fuel and bounds no memory) and starts the page's compile; a seat lands once the page instantiated it, and every call lands before it returns.

#[cfg(target_family = "wasm")]
mod page;

#[cfg(target_family = "wasm")]
pub use page::WebRunner;

#[cfg_attr(
    not(target_family = "wasm"),
    allow(
        dead_code,
        reason = "the browser's runner reads it; its test runs natively"
    )
)]
const DESCRIBE_MEMORY: u64 = 64 << 20;

#[cfg_attr(
    not(target_family = "wasm"),
    allow(
        dead_code,
        reason = "the browser's runner reads it; its test runs natively"
    )
)]
const DESCRIBE_FUEL: u64 = 250_000_000;

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    #[test]
    fn a_describe_module_is_bounded_as_program_describe_bounds_it() {
        assert_eq!(super::DESCRIBE_MEMORY, describe::host::MEMORY as u64);
        assert_eq!(super::DESCRIBE_FUEL, describe::host::FUEL);
    }
}
