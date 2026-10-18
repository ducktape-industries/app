//! pushd is its own compile child (`ducktape_host::runner::COMPILE_APART`),
//! as the desktop app is: a notifier's code compiles in it, into the cache
//! pushd then reads, and the compile's memory leaves with the child.

use std::io::Write as _;
use std::process::{Command, Stdio};

#[test]
fn pushd_compiles_a_notifier_apart_into_the_cache() {
    let cache = tempfile::tempdir().unwrap();
    let compile = || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pushd"))
            .arg(ducktape_host::runner::COMPILE_APART)
            .arg("1")
            .arg(cache.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"\0asm\x01\0\0\0")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    };
    assert_eq!(compile(), "cold");
    assert_eq!(compile(), "disk", "the second compile reads the first");
}
