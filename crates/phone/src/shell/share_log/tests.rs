use super::*;
use ducktape_host::contract::{PickDone, Unkeep};
use std::io::Write as _;
use std::sync::Mutex;

#[derive(Default)]
struct Handed {
    kept: Mutex<Vec<(PathBuf, String)>>,
    answers: Mutex<Vec<KeepDone>>,
}

impl Files for Handed {
    fn pick(&self, _multiple: bool, done: PickDone) {
        done(Ok(None));
    }

    fn keep(&self, path: &Path, done: KeepDone) -> Unkeep {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        self.kept.lock().unwrap().push((path.to_path_buf(), text));
        self.answers.lock().unwrap().push(done);
        Box::new(|| {})
    }
}

#[test]
fn a_shared_log_hands_the_os_both_launches_and_its_copy_is_gone_once_answered() {
    let dir = tempfile::tempdir().unwrap();
    let log = Launches::in_dir(&dir.path().join("logs"));
    writeln!(log.begin().unwrap(), "the launch before's line").unwrap();
    writeln!(log.begin().unwrap(), "this launch's line").unwrap();
    let (cache, files) = (dir.path().join("cache"), Handed::default());
    let (told, heard) = std::sync::mpsc::channel();
    let done: KeepDone = Box::new(move |answer| told.send(answer).unwrap());
    hand(
        prepare(&log, &cache, "ducktape-test.log").unwrap(),
        &files,
        done,
    );

    let kept = files.kept.lock().unwrap().clone();
    let [(path, text)] = kept.as_slice() else {
        panic!("handed the OS {kept:?}");
    };
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("ducktape-test.log")
    );
    assert!(path.starts_with(&cache), "{path:?} is not under the cache");
    assert_eq!(text, "the launch before's line\nthis launch's line\n");
    assert!(path.exists(), "the copy went before the OS answered");
    assert!(heard.try_recv().is_err(), "answered before the OS did");

    let answer = files.answers.lock().unwrap().pop().unwrap();
    answer(Ok(true));
    assert_eq!(heard.recv().unwrap(), Ok(true));
    let folder = path.parent().unwrap();
    assert!(!folder.exists(), "the copy's folder outlived the answer");
    let this = std::fs::read_to_string(dir.path().join("logs/app.log")).unwrap();
    assert_eq!(this, "this launch's line\n", "sharing touched the log");
}

#[test]
fn a_shared_logs_name_says_the_phone_and_the_utc_minute() {
    assert_eq!(
        name("iPhone · iOS 18.1", 31_588_200),
        "ducktape-iPhone-iOS-18.1-1-Jan-1971-1430Z.log"
    );
}
