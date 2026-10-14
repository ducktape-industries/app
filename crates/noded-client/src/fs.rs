//! Files written whole or not at all.

use std::path::Path;

/// Writes `bytes` to `path` whole or not at all: into a temp file of its
/// own beside it, synced, renamed over it, then the directory synced. A
/// crash or a full disk mid-write leaves the old file (or none), never a
/// torn one; two writers racing each land whole, the last rename wins.
/// `private`: only this user can read it (unix).
pub fn atomic_write(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write as _;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?
        .to_string_lossy();
    let pid = std::process::id().to_string();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", crate::ids::fresh_id(&pid)));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    let written = options
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written?;
    // the rename itself survives a power cut only once its directory is synced
    #[cfg(unix)]
    std::fs::File::open(
        path.parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A write cut short (here by a file-size limit of 0 in a child run of
    /// this test, the way a full disk cuts it) leaves the file it was
    /// replacing whole; one that lands replaces it, readable by its owner
    /// only, and leaves no temp behind.
    #[cfg(unix)]
    #[test]
    fn a_write_cut_short_leaves_the_old_file_whole() {
        const CUT: &str = "DUCKTAPE_TEST_CUT_WRITE";
        if let Some(path) = std::env::var_os(CUT) {
            let cut = atomic_write(Path::new(&path), &[1u8; 32], true);
            assert!(cut.is_err(), "the write was not cut short");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        std::fs::write(&path, [7u8; 32]).unwrap();
        let test = concat!(
            module_path!(),
            "::a_write_cut_short_leaves_the_old_file_whole"
        );
        let test = test.split_once("::").unwrap().1;
        let child = std::process::Command::new("sh")
            .args([
                "-c",
                r#"trap "" XFSZ; ulimit -f 0 && exec "$0" --exact "$1" --test-threads=1"#,
            ])
            .arg(std::env::current_exe().unwrap())
            .arg(test)
            .env(CUT, &path)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success() && said.contains("1 passed"),
            "{}: {said}",
            child.status
        );
        assert_eq!(std::fs::read(&path).unwrap(), [7u8; 32], "torn");
        assert_eq!(only_files(dir.path()), ["device.key"]);

        atomic_write(&path, &[1u8; 32], true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), [1u8; 32]);
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "readable by others: {mode:o}");
        assert_eq!(only_files(dir.path()), ["device.key"]);
    }

    #[cfg(unix)]
    fn only_files(dir: &Path) -> Vec<String> {
        let names = std::fs::read_dir(dir).unwrap();
        let mut names: Vec<_> = names
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Writers racing on one file each write a temp of their own: every
    /// write lands whole and says so, and the file ends as one writer's
    /// bytes, never another's half or nothing.
    #[cfg(unix)]
    #[test]
    fn writers_racing_on_one_file_each_land_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device.key");
        let writers: Vec<_> = (1..=8u8)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    (0..100)
                        .map(|_| atomic_write(&path, &[writer; 4096], true))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for writer in writers {
            for written in writer.join().unwrap() {
                written.unwrap();
            }
        }
        let kept = std::fs::read(&path).unwrap();
        assert_eq!(kept.len(), 4096, "torn");
        assert!(kept.iter().all(|byte| *byte == kept[0]), "mixed");
        assert_eq!(only_files(dir.path()), ["device.key"]);
    }
}
