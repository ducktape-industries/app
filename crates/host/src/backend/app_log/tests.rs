use super::*;

fn logged(file: File, said: &'static str) {
    let subscriber = tracing_subscriber::registry().with(file_layer(file));
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "ducktape::app", "{said}");
    });
}

#[test]
fn a_launch_keeps_the_one_before_it_and_a_copy_holds_both_in_the_logs_format() {
    let dir = std::env::temp_dir().join(format!("ducktape-app-log-{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let launches = Launches::in_dir(&dir.join("logs"));
    logged(launches.begin().unwrap(), "the first launch's line");
    logged(launches.begin().unwrap(), "the second launch's line");
    launches.copy_to(&dir.join("both.log")).unwrap();
    let both = std::fs::read_to_string(dir.join("both.log")).unwrap();
    let lines: Vec<&str> = both.lines().collect();
    assert_eq!(lines.len(), 2, "{both}");
    assert!(
        lines[0].ends_with("ducktape::app: the first launch's line"),
        "{both}"
    );
    assert!(
        lines[1].ends_with("ducktape::app: the second launch's line"),
        "{both}"
    );
    assert!(lines.iter().all(|line| line.contains(" INFO ")), "{both}");
    assert!(!both.contains('\u{1b}'), "colour in the log: {both}");

    logged(launches.begin().unwrap(), "the third launch's line");
    launches.copy_to(&dir.join("last.log")).unwrap();
    let last = std::fs::read_to_string(dir.join("last.log")).unwrap();
    assert!(!last.contains("first launch"), "{last}");
    assert!(
        last.contains("second launch") && last.contains("third launch"),
        "{last}"
    );
    assert!(
        launches.copy_to(&dir.join("last.log")).is_err(),
        "a copy wrote over a file already there"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
