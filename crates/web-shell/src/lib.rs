//! ducktape in a browser: the host on gpui's web platform and the page's own WebAssembly, one program's view on the page.
//! A read-only preview: the page reads the network with a throwaway key and signs nothing.

mod link;
#[cfg(target_family = "wasm")]
mod node;
#[cfg(target_family = "wasm")]
mod os;
#[cfg(target_family = "wasm")]
mod page;
