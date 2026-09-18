//! THE BUNDLE IS BUILT FROM THIS REPOSITORY ALONE. `app/README.md` tells a
//! person to run `ops/…` scripts directly, so each one it names is executable
//! as checked out; and every `ops/…` file the bundle script reads is here,
//! not left behind in the repository it was split from.
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

#[test]
fn the_documented_scripts_run_from_a_clean_checkout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
    };

    let readme = read("app/README.md");
    let documented: Vec<String> = std::fs::read_dir(root.join("ops"))
        .unwrap()
        .map(|entry| format!("ops/{}", entry.unwrap().file_name().to_string_lossy()))
        .filter(|path| readme.contains(path.as_str()))
        .collect();
    assert!(
        documented
            .iter()
            .any(|path| path == "ops/bundle-app-macos.sh"),
        "the walk did not find ops/bundle-app-macos.sh in app/README.md: {documented:?}"
    );
    let not_executable: Vec<&String> = documented
        .iter()
        .filter(|path| {
            let mode = std::fs::metadata(root.join(path))
                .unwrap()
                .permissions()
                .mode();
            mode & 0o111 == 0
        })
        .collect();
    assert_eq!(
        not_executable,
        [] as [&String; 0],
        "app/README.md runs these directly"
    );

    let script = read("ops/bundle-app-macos.sh");
    let reads: Vec<&str> = script
        .match_indices("ops/")
        .map(|(at, _)| {
            let token = &script[at..];
            let end = token
                .find(|c: char| !(c.is_ascii_alphanumeric() || "._/-".contains(c)))
                .unwrap_or(token.len());
            token[..end].trim_end_matches('.')
        })
        .collect();
    assert!(
        !reads.is_empty(),
        "the walk found no ops/ path in the bundle script"
    );
    let missing: Vec<&str> = reads
        .into_iter()
        .filter(|path| !root.join(path).is_file())
        .collect();
    assert_eq!(
        missing,
        [] as [&str; 0],
        "ops/bundle-app-macos.sh reads files this repository does not hold"
    );
}
