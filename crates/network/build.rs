//! Bakes in what the binary installs: the core and modules commits the app's Cargo.lock pins
//! (`src/pins.rs`) and the target triple core's published node is named for.
use std::{env, fs, path::Path};

#[path = "src/pins.rs"]
mod pins;

fn main() {
    let lock = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock.display());
    let lock = fs::read_to_string(&lock).expect("the app workspace's Cargo.lock");
    for (repo, var) in [
        ("ducktape", "DUCKTAPE_CORE"),
        ("modules", "DUCKTAPE_MODULES"),
    ] {
        match pins::pinned(&lock, repo) {
            Ok(commit) => println!("cargo:rustc-env={var}={commit}"),
            Err(error) => panic!("{error}"),
        }
    }
    println!(
        "cargo:rustc-env=DUCKTAPE_TARGET={}",
        env::var("TARGET").unwrap()
    );
}
