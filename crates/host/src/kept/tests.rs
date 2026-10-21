//! A keeper's file: what one launch keeps for each network the next reads back, a record equal to
//! the one kept is not written again, a flush writes the newest at once, and a file that does not
//! read is set aside.
use super::Keeper;
use gpui_kit::TestAppContext;

fn words(said: &[&str]) -> Vec<String> {
    said.iter().map(|word| (*word).to_owned()).collect()
}

#[gpui_kit::test]
fn what_one_launch_keeps_for_each_network_the_next_reads_back(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join("kept.json");
    let mut kept = Keeper::<Vec<String>>::at(path.clone());
    cx.update(|cx| {
        kept.keep("one#01010101", words(&["first"]), cx);
        kept.keep("one#01010101", words(&["second"]), cx);
        kept.keep("two#02020202", words(&["other"]), cx);
    });
    cx.run_until_parked();
    let read = Keeper::<Vec<String>>::at(path.clone());
    assert_eq!(
        read.of("one#01010101"),
        Some(&words(&["second"])),
        "the newest record is the one kept"
    );
    assert_eq!(read.of("two#02020202"), Some(&words(&["other"])));
    assert_eq!(read.of("three#03030303"), None);

    std::fs::remove_file(&path).expect("removed");
    cx.update(|cx| kept.keep("two#02020202", words(&["other"]), cx));
    cx.run_until_parked();
    assert!(
        !path.exists(),
        "a record equal to the one kept was written again"
    );

    cx.update(|cx| kept.keep("two#02020202", words(&["flushed"]), cx));
    kept.flush();
    let read = Keeper::<Vec<String>>::at(path.clone());
    assert_eq!(
        read.of("two#02020202"),
        Some(&words(&["flushed"])),
        "a flush writes the newest record before the write it queued runs"
    );
    cx.run_until_parked();
    let read = Keeper::<Vec<String>>::at(path.clone());
    assert_eq!(read.of("two#02020202"), Some(&words(&["flushed"])));
}

#[test]
fn a_file_that_does_not_read_is_set_aside_and_nothing_is_read_from_it() {
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join("kept.json");
    std::fs::write(&path, b"not a record").expect("written");
    let read = Keeper::<Vec<String>>::at(path.clone());
    assert_eq!(read.of("one#01010101"), None);
    assert!(
        dir.path().join("kept.json.bad").exists(),
        "the unreadable file was not set aside"
    );
    assert!(!path.exists());
}
