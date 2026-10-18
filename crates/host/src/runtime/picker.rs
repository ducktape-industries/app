//! `file.pick`: files the person picks on this device, in the system's own
//! picker, for one view to put on the network (`op.put`, a
//! `BlobBody::Picked` naming a slice of one). Only the person's press or
//! key opens the picker (`needs_gesture`), as with the clipboard; it opens
//! on the window thread after the tick. The files picked are opened on a
//! blocking thread and held for the view, by handle, until it lets go of
//! them (`file.release`) or goes away. Files pasted (`clipboard.files`)
//! or dropped on the view (`Event::FileDrop`) are held the same way, a
//! pasted picture as its bytes. `op.put` reads them a slice at a time
//! ([`slice`]), at most `BLOB_MOST` each, and the bytes go to the node,
//! never through the view. A view gets each file's own name, never its
//! path.
//!
//! `file.save`: blobs kept on this device as one file. Only the person's
//! press or key opens the system's save dialog; the blobs go from the
//! node to the file chosen (`kernel::spawn_save`), never through the view.
#[cfg(feature = "ui")]
use super::Seat;
#[cfg(feature = "ui")]
use super::kernel;
use super::wire::methods::{self, Capability, refusal};
use super::{View, wire};
#[cfg(feature = "ui")]
use gpui_kit::{Context, PathPromptOptions};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
#[cfg(any(feature = "ui", test))]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct Picker {
    pending: Vec<(u64, methods::Pick)>,
    saves: Vec<(u64, methods::SaveFile)>,
    /// every file the view holds: picked, pasted or dropped
    held: Held,
}

/// The files one view holds, by handle, shared with the threads that open
/// them.
#[derive(Clone, Default)]
pub(super) struct Held(Arc<Mutex<Open>>);

#[derive(Default)]
struct Open {
    /// the handle the next file held is given: never one given before
    next: u64,
    files: HashMap<u64, Arc<Picked>>,
}

/// One file the view holds.
pub(super) struct Picked {
    name: String,
    /// its size as held: a file of another size since is not the one the
    /// person picked
    pub(super) size: u64,
    source: Source,
}

/// Where a held file's bytes are.
enum Source {
    /// a file on this device, open
    File(Mutex<File>),
    /// bytes the host holds itself: a picture pasted from the clipboard
    Bytes(Arc<[u8]>),
}

/// A file opened, or bytes taken, not held yet ([`Held::hold`]).
pub(super) struct Opened {
    name: String,
    size: u64,
    source: Source,
}

impl Opened {
    /// Bytes the host holds itself, named `name`.
    #[cfg(any(feature = "ui", test))]
    pub(super) fn bytes(name: String, bytes: Vec<u8>) -> Self {
        Self {
            name,
            size: bytes.len() as u64,
            source: Source::Bytes(bytes.into()),
        }
    }
}

impl Held {
    /// `files` held, each under a new handle, as the view is told them.
    #[cfg(any(feature = "ui", test))]
    pub(super) fn hold(&self, files: Vec<Opened>) -> Vec<methods::Picked> {
        let open = &mut *self.0.lock().expect("held files");
        files
            .into_iter()
            .map(|Opened { name, size, source }| {
                open.next += 1;
                let picked = Picked {
                    name: name.clone(),
                    size,
                    source,
                };
                open.files.insert(open.next, Arc::new(picked));
                methods::Picked {
                    name,
                    size,
                    file: open.next,
                }
            })
            .collect()
    }

    fn get(&self, handle: u64) -> Option<Arc<Picked>> {
        let open = self.0.lock().expect("held files");
        open.files.get(&handle).cloned()
    }

    /// Lets go of each of `handles`; one the view does not hold is passed
    /// over. A put reading one already goes on: it holds the file itself.
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

    /// The file `handle` names, if the view holds it.
    pub(super) fn picked(&self, handle: u64) -> Option<Arc<Picked>> {
        self.held.get(handle)
    }

    /// The files the view holds, for a thread that holds more.
    #[cfg(feature = "ui")]
    pub(super) fn held(&self) -> Held {
        self.held.clone()
    }

    /// `paths` picked, as the system's picker would answer them.
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
        (Capability::File, "release") => release(view, id, payload),
        (Capability::File, "save") => save(view, id, payload),
        _ => return false,
    }
    true
}

/// What is on this device leaves it only by the person's own pick, and
/// only a press or key opens the picker.
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

/// The view lets go of files it holds: no press needed, it only closes
/// them.
fn release(view: &mut View, id: u64, payload: &[u8]) {
    match methods::decode::<Vec<u64>>(payload) {
        Ok(handles) => {
            view.picker.held.release(&handles);
            view.reply(id, Ok(methods::encode(&())));
        }
        Err(error) => view.refuse(id, refusal::MALFORMED_REQUEST, error),
    }
}

/// A file is written on this device only where the person chooses, and
/// only a press or key opens the dialog that asks.
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

/// Opens the system's picker for each pick asked since the last turn, and
/// its save dialog for each save; each is answered once the person picks,
/// chooses or cancels. A host with no window (pushd) opens none: its
/// notifiers have no press to pick with. A phone's shell has no save
/// dialog yet (gpui-pre's iOS and Android platforms answer none), so a
/// save there is refused saying so.
#[cfg(feature = "ui")]
pub(super) fn mount(view: &mut View, cx: &mut Context<Seat>) {
    for (id, pick) in std::mem::take(&mut view.picker.pending) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: pick.multiple,
            prompt: None,
        });
        let held = view.picker.held.clone();
        kernel::spawn_reply(view, id, async move {
            let paths = match prompt.await {
                // the picker went without an answer: as cancelled
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
        if cfg!(any(target_os = "ios", target_os = "android")) {
            view.refuse(id, refusal::HOST_FAULT, "this device has no save dialog");
            continue;
        }
        if !kernel::reaches_node(view, id) {
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
                // the dialog went without an answer: as cancelled
                Err(_) => Ok(None),
                Ok(Err(error)) => Err(fault(error.to_string())),
                Ok(Ok(path)) => Ok(path),
            }
        });
    }
}

/// Files dropped on a node of the view that listens for them
/// (`on_file_drop`): opened on a blocking thread and held as a pick's
/// are, then told to the view (`Event::FileDrop`). A drop that is not all
/// files, or one that cannot be opened, is passed over whole and logged
/// by its name alone: the view asked nothing it could be refused.
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

/// The files at `paths`, each opened and named by its own name and size,
/// in order. A path that is not a file, or that cannot be opened, refuses
/// them all by its name alone.
#[cfg(any(feature = "ui", test))]
pub(super) fn open_paths(paths: Vec<PathBuf>) -> Result<Vec<Opened>, wire::Error> {
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let named = |error: std::io::Error| fault(format!("{name}: {error}"));
        let not_a_file = || fault(format!("{name} is not a file"));
        // looked at before it is opened: opening a pipe waits for a writer
        if !std::fs::metadata(&path).map_err(named)?.is_file() {
            return Err(not_a_file());
        }
        let file = File::open(&path).map_err(named)?;
        let metadata = file.metadata().map_err(named)?;
        if !metadata.is_file() {
            return Err(not_a_file());
        }
        files.push(Opened {
            name,
            size: metadata.len(),
            source: Source::File(Mutex::new(file)),
        });
    }
    Ok(files)
}

/// The `len` bytes of a held file from `offset` (the caller keeps them
/// within its size as held). A file whose size changed since it was
/// picked is refused: its slices would mix two files.
pub(super) fn slice(picked: &Picked, offset: u64, len: u64) -> Result<Vec<u8>, wire::Error> {
    let file = match &picked.source {
        Source::Bytes(bytes) => return Ok(bytes[offset as usize..][..len as usize].to_vec()),
        Source::File(file) => file,
    };
    let named = |error: std::io::Error| fault(format!("{}: {error}", picked.name));
    let file = &mut *file.lock().expect("a picked file");
    if file.metadata().map_err(named)?.len() != picked.size {
        return Err(fault(format!(
            "{} changed since it was picked",
            picked.name
        )));
    }
    file.seek(SeekFrom::Start(offset)).map_err(named)?;
    let mut bytes = vec![0; len as usize];
    file.read_exact(&mut bytes).map_err(named)?;
    Ok(bytes)
}

pub(super) fn fault(message: String) -> wire::Error {
    wire::Error::new(refusal::HOST_FAULT, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(held: &Held, file: u64, offset: u64, len: u64) -> Vec<u8> {
        slice(&held.get(file).expect("held"), offset, len).expect("read")
    }

    #[test]
    fn a_file_is_named_without_its_folder_and_read_a_slice_at_a_time() {
        const MOST: u64 = methods::BLOB_MOST;
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

    #[test]
    fn bytes_the_host_holds_read_as_a_file_does() {
        let held = Held::default();
        let picture = Opened::bytes("image.png".into(), b"picture".to_vec());
        let picked = held.hold(vec![picture]);
        assert_eq!((picked[0].name.as_str(), picked[0].size), ("image.png", 7));
        assert_eq!(read(&held, 1, 2, 3), b"ctu");
    }

    #[test]
    fn a_file_that_changed_size_since_it_was_picked_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        std::fs::write(&path, b"one").unwrap();
        let held = Held::default();
        held.hold(open_paths(vec![path.clone()]).unwrap());
        std::fs::write(&path, b"one two").unwrap();
        let changed = slice(&held.get(1).unwrap(), 0, 3).unwrap_err();
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
