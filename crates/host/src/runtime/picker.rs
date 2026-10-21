//! `file.pick`: files the person picks on this device, in the system's own
//! picker, for one view to put on the network (`op.put`, a
//! `BlobBody::Picked` naming a slice of one). Only the person's press or
//! key opens the picker (`needs_gesture`), as with the clipboard; it opens
//! on the window thread after the tick. The files picked are opened on a
//! blocking thread and held for the view, by handle, until it lets go of
//! them (`file.release`) or goes away. Files pasted (`clipboard.files`)
//! or dropped on the view (`Event::FileDrop`) are held the same way, a
//! pasted picture in an unnamed file of its own, so the host's memory
//! never holds what a view keeps. `op.put` reads each slice it names a
//! part at a time ([`read`]) and sends it to the node; `file.read` hands
//! the view the range it asks of one. A view gets each file's own name,
//! never its path.
//!
//! `file.save`: blobs kept on this device as one file. Only the person's
//! press or key opens the system's save dialog; the blobs go from the
//! node to the file chosen (`kernel::spawn_save`), never through the view.
//!
//! A shell with dialogs of its own (`contract::Files`, the phones') picks
//! and keeps in place of gpui: a pick answers the shell's copies, which the
//! host opens and then removes; a save writes the file in a folder of its
//! own under the cache, hands it to the shell to keep, and removes the
//! folder once it answers (`kernel::spawn_keep`).
#[cfg(feature = "ui")]
use super::Seat;
use super::kernel;
use super::wire::methods::{self, Capability, refusal};
use super::{View, wire};
#[cfg(any(feature = "ui", test))]
use crate::contract::Files;
#[cfg(feature = "ui")]
use gpui_kit::{Context, PathPromptOptions};
use std::collections::HashMap;
use std::fs::File;
#[cfg(any(feature = "ui", test))]
use std::io::Write;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
#[cfg(any(feature = "ui", test))]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct Picker {
    pending: Vec<(u64, methods::Pick)>,
    saves: Vec<(u64, methods::SaveFile)>,
    held: Held,
}

#[derive(Clone, Default)]
pub(super) struct Held(Arc<Mutex<Open>>);

#[derive(Default)]
struct Open {
    next: u64,
    files: HashMap<u64, Arc<Picked>>,
}

pub(super) struct Picked {
    name: String,
    pub(super) size: u64,
    file: Mutex<File>,
}

impl Picked {
    #[cfg(any(feature = "ui", test))]
    pub(super) fn bytes(name: String, bytes: &[u8]) -> Result<Self, wire::Error> {
        let named = |error: std::io::Error| fault(format!("{name}: {error}"));
        let mut file = tempfile::tempfile().map_err(named)?;
        file.write_all(bytes).map_err(named)?;
        Ok(Self {
            size: bytes.len() as u64,
            file: Mutex::new(file),
            name,
        })
    }
}

impl Held {
    #[cfg(any(feature = "ui", test))]
    pub(super) fn hold(&self, files: Vec<Picked>) -> Vec<methods::Picked> {
        let open = &mut *self.0.lock().expect("held files");
        files
            .into_iter()
            .map(|picked| {
                open.next += 1;
                let told = methods::Picked {
                    name: picked.name.clone(),
                    size: picked.size,
                    file: open.next,
                };
                open.files.insert(open.next, Arc::new(picked));
                told
            })
            .collect()
    }

    fn get(&self, handle: u64) -> Option<Arc<Picked>> {
        let open = self.0.lock().expect("held files");
        open.files.get(&handle).cloned()
    }

    fn release(&self, handles: &[u64]) {
        let open = &mut *self.0.lock().expect("held files");
        for handle in handles {
            open.files.remove(handle);
        }
    }
}

impl Picker {
    pub(super) fn cancel(&mut self, id: u64) {
        self.pending.retain(|(pending, _)| *pending != id);
        self.saves.retain(|(pending, _)| *pending != id);
    }

    pub(super) fn picked(&self, handle: u64) -> Option<Arc<Picked>> {
        self.held.get(handle)
    }

    #[cfg(feature = "ui")]
    pub(super) fn held(&self) -> Held {
        self.held.clone()
    }

    #[cfg(test)]
    pub(super) fn pick(&self, paths: Vec<PathBuf>) -> Vec<methods::Picked> {
        self.held.hold(open_paths(paths).expect("opened"))
    }
}

pub(super) fn answer(
    view: &mut View,
    capability: Capability,
    operation: &str,
    id: u64,
    payload: &[u8],
) -> bool {
    match (capability, operation) {
        (Capability::File, "pick") => pick(view, id, payload),
        (Capability::File, "read") => read_file(view, id, payload),
        (Capability::File, "release") => release(view, id, payload),
        (Capability::File, "save") => save(view, id, payload),
        _ => return false,
    }
    true
}

fn pick(view: &mut View, id: u64, payload: &[u8]) {
    match methods::decode::<methods::Pick>(payload) {
        Ok(_) if !view.take_activation() => view.refuse(
            id,
            refusal::NEEDS_GESTURE,
            "`file.pick` needs a press or key",
        ),
        Ok(pick) => view.picker.pending.push((id, pick)),
        Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
}

fn read_file(view: &mut View, id: u64, payload: &[u8]) {
    let ask = match methods::decode::<methods::ReadFile>(payload) {
        Ok(ask) => ask,
        Err(error) => return view.refuse(id, refusal::MALFORMED_REQUEST, error),
    };
    let Some(picked) = view.picker.picked(ask.file) else {
        let unheld = format!("file {} is not one the view holds", ask.file);
        return view.refuse(id, refusal::MALFORMED_REQUEST, unheld);
    };
    let end = ask.offset.checked_add(ask.len);
    if end.is_none_or(|end| end > picked.size) {
        let past = format!(
            "{} bytes from {} are past the end of file {}, {} bytes",
            ask.len, ask.offset, ask.file, picked.size
        );
        return view.refuse(id, refusal::MALFORMED_REQUEST, past);
    }
    kernel::spawn_reply(view, id, async move {
        let read = tokio::task::spawn_blocking(move || read_whole(&picked, ask.offset, ask.len));
        let bytes = read
            .await
            .unwrap_or_else(|error| Err(fault(error.to_string())))?;
        Ok(methods::encode(&bytes))
    });
}

fn read_whole(picked: &Picked, offset: u64, len: u64) -> Result<Vec<u8>, wire::Error> {
    let mut bytes = Vec::new();
    read(picked, offset, len, |part| {
        bytes.extend_from_slice(part);
        true
    })?;
    Ok(bytes)
}

fn release(view: &mut View, id: u64, payload: &[u8]) {
    match methods::decode::<Vec<u64>>(payload) {
        Ok(handles) => {
            view.picker.held.release(&handles);
            view.reply(id, Ok(methods::encode(&())));
        }
        Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
}

fn save(view: &mut View, id: u64, payload: &[u8]) {
    match methods::decode::<methods::SaveFile>(payload) {
        Ok(_) if !view.take_activation() => view.refuse(
            id,
            refusal::NEEDS_GESTURE,
            "`file.save` needs a press or key",
        ),
        Ok(save) => view.picker.saves.push((id, save)),
        Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
}

#[cfg(feature = "ui")]
pub(super) fn mount(view: &mut View, cx: &mut Context<Seat>) {
    for (id, pick) in std::mem::take(&mut view.picker.pending) {
        if let Some(files) = crate::contract::os().files() {
            let picked = shell_pick(files, pick.multiple);
            let held = view.picker.held.clone();
            kernel::spawn_reply(view, id, async move {
                let paths = picked.await?;
                let files = tokio::task::spawn_blocking(move || open_copies(paths))
                    .await
                    .unwrap_or_else(|error| Err(fault(error.to_string())))?;
                Ok(methods::encode(&held.hold(files)))
            });
            continue;
        }
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: pick.multiple,
            prompt: None,
        });
        let held = view.picker.held.clone();
        kernel::spawn_reply(view, id, async move {
            let paths = match prompt.await {
                Err(_) => Vec::new(),
                Ok(Err(error)) => return Err(fault(error.to_string())),
                Ok(Ok(paths)) => paths.unwrap_or_default(),
            };
            let files = tokio::task::spawn_blocking(move || open_paths(paths))
                .await
                .unwrap_or_else(|error| Err(fault(error.to_string())))?;
            Ok(methods::encode(&held.hold(files)))
        });
    }
    for (id, save) in std::mem::take(&mut view.picker.saves) {
        if !kernel::reaches_node(view, id) {
            continue;
        }
        if let Some(files) = crate::contract::os().files() {
            let path = keeping_path(&crate::contract::os().dirs().cache, &save.name);
            kernel::spawn_keep(view, id, save, path, files);
            continue;
        }
        let home = std::env::home_dir().unwrap_or_default();
        let downloads = home.join("Downloads");
        let folder = match downloads.is_dir() {
            true => downloads,
            false => home,
        };
        let prompt = cx.prompt_for_new_path(&folder, Some(&save.name));
        kernel::spawn_save(view, id, save, async move {
            match prompt.await {
                Err(_) => Ok(None),
                Ok(Err(error)) => Err(fault(error.to_string())),
                Ok(Ok(path)) => Ok(path),
            }
        });
    }
}

#[cfg(feature = "ui")]
pub(super) fn dropped(view: &View, handler: u32, paths: Vec<PathBuf>) {
    let held = view.picker.held.clone();
    kernel::spawn_event(view, async move {
        let opened = tokio::task::spawn_blocking(move || open_paths(paths))
            .await
            .unwrap_or_else(|error| Err(fault(error.to_string())));
        match opened {
            Ok(files) => Some(wire::Event::FileDrop {
                handler,
                files: held.hold(files),
            }),
            Err(refusal) => {
                tracing::warn!(
                    target: "ducktape::app",
                    reason = "file_drop_unopened",
                    error = %refusal.message,
                    "files dropped on a view were not opened"
                );
                None
            }
        }
    });
}

#[cfg(any(feature = "ui", test))]
pub(super) fn open_paths(paths: Vec<PathBuf>) -> Result<Vec<Picked>, wire::Error> {
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let named = |error: std::io::Error| fault(format!("{name}: {error}"));
        let not_a_file = || fault(format!("{name} is not a file"));
        if !std::fs::metadata(&path).map_err(named)?.is_file() {
            return Err(not_a_file());
        }
        let file = File::open(&path).map_err(named)?;
        let metadata = file.metadata().map_err(named)?;
        if !metadata.is_file() {
            return Err(not_a_file());
        }
        files.push(Picked {
            name,
            size: metadata.len(),
            file: Mutex::new(file),
        });
    }
    Ok(files)
}

#[cfg(any(feature = "ui", test))]
fn shell_pick(
    files: &dyn Files,
    multiple: bool,
) -> impl std::future::Future<Output = Result<Vec<PathBuf>, wire::Error>> + use<> {
    let (picked, answer) = tokio::sync::oneshot::channel();
    files.pick(
        multiple,
        Box::new(move |paths| {
            let _ = picked.send(paths);
        }),
    );
    async move {
        match answer.await {
            Err(_) | Ok(Ok(None)) => Ok(Vec::new()),
            Ok(Ok(Some(paths))) => Ok(paths),
            Ok(Err(error)) => Err(fault(error)),
        }
    }
}

#[cfg(any(feature = "ui", test))]
fn open_copies(paths: Vec<PathBuf>) -> Result<Vec<Picked>, wire::Error> {
    let opened = open_paths(paths.clone());
    for path in &paths {
        let _ = std::fs::remove_file(path);
        if let Some(folder) = path.parent() {
            let _ = std::fs::remove_dir(folder);
        }
    }
    opened
}

#[cfg(any(feature = "ui", test))]
fn keeping_path(cache: &std::path::Path, name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = std::path::Path::new(name)
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let folder = format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    cache.join("kept").join(folder).join(name)
}

/// A folder of the host's own under the cache holding a file to keep, removed when dropped.
#[cfg(any(feature = "ui", test))]
pub(in crate::runtime) struct KeptFolder(pub(in crate::runtime) PathBuf);

#[cfg(any(feature = "ui", test))]
impl Drop for KeptFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(any(feature = "ui", test))]
struct UnkeptOnDrop(Option<crate::contract::Unkeep>);

#[cfg(any(feature = "ui", test))]
impl Drop for UnkeptOnDrop {
    fn drop(&mut self) {
        if let Some(unkeep) = self.0.take() {
            unkeep();
        }
    }
}

#[cfg(any(feature = "ui", test))]
pub(in crate::runtime) async fn keep(
    files: &dyn Files,
    path: &std::path::Path,
    folder: KeptFolder,
) -> Result<Vec<u8>, wire::Error> {
    let (kept, answer) = tokio::sync::oneshot::channel();
    let unkeep = files.keep(
        path,
        Box::new(move |done| {
            drop(folder);
            let _ = kept.send(done);
        }),
    );
    let _unkept_on_drop = UnkeptOnDrop(Some(unkeep));
    match answer.await {
        Err(_) => Ok(methods::encode(&false)),
        Ok(Ok(done)) => Ok(methods::encode(&done)),
        Ok(Err(error)) => Err(fault(error)),
    }
}

pub(super) fn read(
    picked: &Picked,
    offset: u64,
    len: u64,
    mut part: impl FnMut(&[u8]) -> bool,
) -> Result<(), wire::Error> {
    let named = |error: std::io::Error| fault(format!("{}: {error}", picked.name));
    let file = &mut *picked.file.lock().expect("a picked file");
    if file.metadata().map_err(named)?.len() != picked.size {
        return Err(fault(format!(
            "{} changed since it was picked",
            picked.name
        )));
    }
    file.seek(SeekFrom::Start(offset)).map_err(named)?;
    let mut reader = BufReader::new(file.take(len));
    let mut left = len;
    while left > 0 {
        let bytes = reader.fill_buf().map_err(named)?;
        if bytes.is_empty() {
            return Err(named(std::io::ErrorKind::UnexpectedEof.into()));
        }
        let taken = bytes.len();
        if !part(bytes) {
            return Ok(());
        }
        reader.consume(taken);
        left -= taken as u64;
    }
    Ok(())
}

pub(super) fn fault(message: String) -> wire::Error {
    wire::Error::new(refusal::HOST_FAULT, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(held: &Held, file: u64, offset: u64, len: u64) -> Vec<u8> {
        read_whole(&held.get(file).expect("held"), offset, len).expect("read")
    }

    #[test]
    fn a_file_is_named_without_its_folder_and_read_a_slice_at_a_time() {
        const MOST: u64 = 1 << 20;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tape.png");
        let bytes: Vec<u8> = (0..MOST + 5).map(|at| at as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let held = Held::default();
        let picked = held.hold(open_paths(vec![path]).unwrap());
        assert_eq!(
            picked,
            [methods::Picked {
                name: "tape.png".into(),
                size: bytes.len() as u64,
                file: 1,
            }]
        );
        assert_eq!(read(&held, 1, 0, MOST), bytes[..MOST as usize]);
        assert_eq!(read(&held, 1, MOST, 5), bytes[MOST as usize..]);
    }

    #[test]
    fn a_second_pick_keeps_the_first_and_a_release_lets_go_of_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        let held = Held::default();
        held.hold(open_paths(vec![a]).unwrap());
        let again = held.hold(open_paths(vec![b]).unwrap());
        assert_eq!(again[0].file, 2, "a handle is never given twice");
        assert_eq!(read(&held, 1, 0, 1), b"a", "the first pick stays");
        assert_eq!(read(&held, 2, 0, 1), b"b");
        held.release(&[1, 7]);
        assert!(held.get(1).is_none(), "released");
        assert_eq!(read(&held, 2, 0, 1), b"b", "the other stays");
    }

    struct Shell {
        picked: Option<Result<Option<Vec<PathBuf>>, String>>,
        kept: &'static [u8],
    }

    impl Files for Shell {
        fn pick(&self, _: bool, done: crate::contract::PickDone) {
            if let Some(picked) = self.picked.clone() {
                done(picked);
            }
        }
        fn keep(
            &self,
            path: &std::path::Path,
            done: crate::contract::KeepDone,
        ) -> crate::contract::Unkeep {
            done(Ok(std::fs::read(path).is_ok_and(|bytes| bytes == self.kept)));
            Box::new(|| {})
        }
    }

    struct ShowsLater {
        done: Mutex<Option<crate::contract::KeepDone>>,
        unkept: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Files for ShowsLater {
        fn pick(&self, _: bool, _: crate::contract::PickDone) {}
        fn keep(
            &self,
            _: &std::path::Path,
            done: crate::contract::KeepDone,
        ) -> crate::contract::Unkeep {
            *self.done.lock().unwrap() = Some(done);
            let unkept = self.unkept.clone();
            Box::new(move || unkept.store(true, std::sync::atomic::Ordering::SeqCst))
        }
    }

    fn kept_folder(root: &std::path::Path, name: &str, bytes: &[u8]) -> (PathBuf, PathBuf) {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("tape.txt");
        std::fs::write(&path, bytes).unwrap();
        (folder, path)
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        let runtime = tokio::runtime::Builder::new_current_thread().build();
        runtime.unwrap().block_on(future)
    }

    #[test]
    fn a_shells_pick_is_opened_and_its_copies_go() {
        let dir = tempfile::tempdir().unwrap();
        let copy = dir.path().join("pick-1").join("tape.png");
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::write(&copy, b"tape").unwrap();
        let shell = Shell {
            picked: Some(Ok(Some(vec![copy.clone()]))),
            kept: b"",
        };
        let paths = run(shell_pick(&shell, true)).unwrap();
        let held = Held::default();
        let picked = held.hold(open_copies(paths).unwrap());
        assert_eq!((picked[0].name.as_str(), picked[0].size), ("tape.png", 4));
        assert_eq!(read(&held, 1, 0, 4), b"tape", "read after its copy went");
        assert!(!copy.parent().unwrap().exists(), "the copy and its folder");
    }

    #[test]
    fn a_shell_pick_cancelled_or_unanswered_picks_nothing_and_a_failed_one_says_why() {
        let shell = |picked| Shell { picked, kept: b"" };
        assert!(
            run(shell_pick(&shell(Some(Ok(None))), false))
                .unwrap()
                .is_empty()
        );
        assert!(run(shell_pick(&shell(None), false)).unwrap().is_empty());
        let failed = run(shell_pick(&shell(Some(Err("no window".into()))), false));
        let failed = failed.unwrap_err();
        assert_eq!(
            (failed.code.as_str(), failed.message.as_str()),
            ("host_fault", "no window")
        );
    }

    #[test]
    fn a_file_to_keep_has_a_folder_of_its_own_and_its_own_last_name() {
        let cache = std::path::Path::new("/cache");
        let first = keeping_path(cache, "../../etc/passwd");
        assert_eq!(first.file_name(), Some("passwd".as_ref()));
        assert!(first.starts_with("/cache/kept"));
        let second = keeping_path(cache, "passwd");
        assert_ne!(first.parent(), second.parent(), "a folder each");
        assert_eq!(keeping_path(cache, "..").file_name(), Some("file".as_ref()));
    }

    #[test]
    fn a_kept_file_answers_the_shells_word_and_its_folder_goes() {
        let dir = tempfile::tempdir().unwrap();
        let shell = |kept| Shell { picked: None, kept };
        let (folder, path) = kept_folder(dir.path(), "kept-1", b"kept");
        let kept = run(keep(&shell(b"kept"), &path, KeptFolder(folder.clone()))).unwrap();
        assert_eq!(kept, methods::encode(&true));
        assert!(!folder.exists(), "the folder goes once the shell answers");
        let (folder, path) = kept_folder(dir.path(), "kept-2", b"kept");
        let cancelled = run(keep(&shell(b"other"), &path, KeptFolder(folder))).unwrap();
        assert_eq!(cancelled, methods::encode(&false));
    }

    #[test]
    fn a_keep_let_go_after_the_hand_off_is_unkept_and_its_folder_goes_once_the_shell_answers() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, path) = kept_folder(dir.path(), "kept-1", b"tape");
        let shell = ShowsLater {
            done: Mutex::new(None),
            unkept: Default::default(),
        };
        let mut keeping = Box::pin(keep(&shell, &path, KeptFolder(folder.clone())));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            keeping.as_mut().poll(&mut cx).is_pending(),
            "handed to the shell"
        );
        drop(keeping);
        assert!(
            shell.unkept.load(std::sync::atomic::Ordering::SeqCst),
            "the shell hears the host let go"
        );
        assert!(
            path.exists(),
            "a viewer the shell already shows still reads it"
        );
        let done = shell
            .done
            .lock()
            .unwrap()
            .take()
            .expect("the shell holds the keep");
        done(Ok(false));
        assert!(!folder.exists(), "the folder goes once the shell answers");
    }

    #[test]
    fn a_keep_let_go_before_the_hand_off_takes_its_folder_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, _) = kept_folder(dir.path(), "kept-1", b"tape");
        drop(KeptFolder(folder.clone()));
        assert!(!folder.exists());
    }

    #[test]
    fn a_pasted_picture_is_held_in_a_file_and_read_as_one() {
        let held = Held::default();
        let picture = Picked::bytes("image.png".into(), b"picture").unwrap();
        let picked = held.hold(vec![picture]);
        assert_eq!((picked[0].name.as_str(), picked[0].size), ("image.png", 7));
        assert_eq!(read(&held, 1, 2, 3), b"ctu");
    }

    #[test]
    fn a_slice_is_read_in_parts_and_stops_when_no_more_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("film.mov");
        let bytes: Vec<u8> = (0..3 << 20).map(|at: u32| (at * 7) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let held = Held::default();
        held.hold(open_paths(vec![path]).unwrap());
        let picked = held.get(1).unwrap();
        let mut parts = Vec::new();
        super::read(&picked, 5, bytes.len() as u64 - 9, |part| {
            parts.push(part.to_vec());
            true
        })
        .unwrap();
        assert!(parts.len() > 1, "no part holds the whole slice");
        assert_eq!(parts.concat(), bytes[5..bytes.len() - 4]);
        let mut first = 0;
        super::read(&picked, 0, bytes.len() as u64, |_| {
            first += 1;
            false
        })
        .unwrap();
        assert_eq!(first, 1, "a part not taken ends the read");
    }

    #[test]
    fn a_file_that_changed_size_since_it_was_picked_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        std::fs::write(&path, b"one").unwrap();
        let held = Held::default();
        held.hold(open_paths(vec![path.clone()]).unwrap());
        std::fs::write(&path, b"one two").unwrap();
        let changed = super::read(&held.get(1).unwrap(), 0, 3, |_| true).unwrap_err();
        assert_eq!(changed.message, "log.txt changed since it was picked");
    }

    #[test]
    fn a_pick_that_cannot_be_opened_is_refused_by_its_name_alone() {
        let dir = tempfile::tempdir().unwrap();
        let Err(gone) = open_paths(vec![dir.path().join("gone.txt")]) else {
            panic!("opened");
        };
        assert_eq!(gone.code, refusal::HOST_FAULT);
        assert!(gone.message.starts_with("gone.txt: "), "{}", gone.message);
        let Err(folder) = open_paths(vec![dir.path().to_path_buf()]) else {
            panic!("opened");
        };
        let where_it_is = dir.path().to_string_lossy();
        assert!(
            !folder.message.contains(&*where_it_is),
            "{}",
            folder.message
        );
    }
}
