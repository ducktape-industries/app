//! `file.pick`: files the person picks on this device, in the system's own
//! picker, for one view to put on the network (`op.put`, a
//! `BlobBody::Picked` naming a slice of one). Only the person's press or
//! key opens the picker (`needs_gesture`), as with the clipboard; it opens
//! on the window thread after the tick. The files picked are opened on a
//! blocking thread and held open for the view, by handle, until it picks
//! files again (a cancelled pick keeps them) or goes away; `op.put` reads
//! them a slice at a time ([`slice`]), at most `BLOB_MOST` each, and the
//! bytes go to the node, never through the view. A view gets each file's
//! own name, never its path.
#[cfg(feature = "ui")]
use super::Seat;
#[cfg(feature = "ui")]
use super::kernel;
#[cfg(any(feature = "ui", test))]
use super::kernel::Answer;
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
    /// the latest pick's files; the pick being answered fills it anew
    open: Arc<Mutex<Open>>,
}

#[derive(Default)]
struct Open {
    /// the handle the next file picked is given: never one given before
    next: u64,
    files: HashMap<u64, Arc<Picked>>,
}

/// One file picked, open.
pub(super) struct Picked {
    name: String,
    /// its size as picked: a file of another size since is not the one
    /// the person picked
    pub(super) size: u64,
    file: Mutex<File>,
}

impl Picker {
    pub(super) fn cancel(&mut self, id: u64) {
        self.pending.retain(|(pending, _)| *pending != id);
    }

    /// The file of the latest pick `handle` names, if it names one.
    pub(super) fn picked(&self, handle: u64) -> Option<Arc<Picked>> {
        self.open
            .lock()
            .expect("picked files")
            .files
            .get(&handle)
            .cloned()
    }

    /// `paths` picked, as the system's picker would answer them.
    #[cfg(test)]
    pub(super) fn pick(&self, paths: Vec<PathBuf>) -> Vec<methods::Picked> {
        methods::decode(&opened(paths, &self.open).expect("opened")).expect("decodes")
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

/// Opens the system's picker for each pick asked since the last turn; each
/// is answered once the person picks or cancels. A host with no window
/// (pushd) opens none: its notifiers have no press to pick with.
#[cfg(feature = "ui")]
pub(super) fn mount(view: &mut View, cx: &mut Context<Seat>) {
    for (id, pick) in std::mem::take(&mut view.picker.pending) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: pick.multiple,
            prompt: None,
        });
        let open = view.picker.open.clone();
        kernel::spawn_reply(view, id, async move {
            let paths = match prompt.await {
                // the picker went without an answer: as cancelled
                Err(_) => Vec::new(),
                Ok(Err(error)) => return Err(fault(error.to_string())),
                Ok(Ok(paths)) => paths.unwrap_or_default(),
            };
            tokio::task::spawn_blocking(move || opened(paths, &open))
                .await
                .unwrap_or_else(|error| Err(fault(error.to_string())))
        });
    }
}

/// The files picked, each opened and named by its own name and size, in
/// place of the pick before; none picked (cancelled), the pick before
/// stays. A pick that is not a file, or that cannot be opened, refuses the
/// whole pick by its name alone.
#[cfg(any(feature = "ui", test))]
fn opened(paths: Vec<PathBuf>, open: &Mutex<Open>) -> Answer {
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
        files.push((name, metadata.len(), file));
    }
    if files.is_empty() {
        return Ok(methods::encode(&Vec::<methods::Picked>::new()));
    }
    let open = &mut *open.lock().expect("picked files");
    open.files.clear();
    let picked: Vec<methods::Picked> = files
        .into_iter()
        .map(|(name, size, file)| {
            open.next += 1;
            let picked = Picked {
                name: name.clone(),
                size,
                file: Mutex::new(file),
            };
            open.files.insert(open.next, Arc::new(picked));
            methods::Picked {
                name,
                size,
                file: open.next,
            }
        })
        .collect();
    Ok(methods::encode(&picked))
}

/// The `len` bytes of a file picked from `offset` (the caller keeps them
/// within its size as picked). A file whose size changed since it was
/// picked is refused: its slices would mix two files.
pub(super) fn slice(picked: &Picked, offset: u64, len: u64) -> Result<Vec<u8>, wire::Error> {
    let named = |error: std::io::Error| fault(format!("{}: {error}", picked.name));
    let file = &mut *picked.file.lock().expect("a picked file");
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

fn fault(message: String) -> wire::Error {
    wire::Error::new(refusal::HOST_FAULT, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(answer: Answer) -> Vec<methods::Picked> {
        methods::decode(&answer.expect("opened")).expect("decodes")
    }

    fn read(open: &Mutex<Open>, file: u64, offset: u64, len: u64) -> Vec<u8> {
        let picked = open.lock().unwrap().files[&file].clone();
        slice(&picked, offset, len).expect("read")
    }

    #[test]
    fn a_file_is_named_without_its_folder_and_read_a_slice_at_a_time() {
        const MOST: u64 = methods::BLOB_MOST;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tape.png");
        let bytes: Vec<u8> = (0..MOST + 5).map(|at| at as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let open = Mutex::default();
        let picked = picked(opened(vec![path], &open));
        assert_eq!(
            picked,
            [methods::Picked {
                name: "tape.png".into(),
                size: bytes.len() as u64,
                file: 1,
            }]
        );
        assert_eq!(read(&open, 1, 0, MOST), bytes[..MOST as usize]);
        assert_eq!(read(&open, 1, MOST, 5), bytes[MOST as usize..]);
    }

    #[test]
    fn a_new_pick_closes_the_files_of_the_one_before_and_a_cancel_keeps_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"a").unwrap();
        let open = Mutex::default();
        picked(opened(vec![path.clone()], &open));
        assert!(picked(opened(Vec::new(), &open)).is_empty());
        assert_eq!(
            read(&open, 1, 0, 1),
            b"a",
            "cancelled: the pick before stays"
        );
        let again = picked(opened(vec![path], &open));
        assert_eq!(again[0].file, 2, "a handle is never given twice");
        assert_eq!(open.lock().unwrap().files.keys().collect::<Vec<_>>(), [&2]);
    }

    #[test]
    fn a_file_that_changed_size_since_it_was_picked_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        std::fs::write(&path, b"one").unwrap();
        let open = Mutex::default();
        picked(opened(vec![path.clone()], &open));
        std::fs::write(&path, b"one two").unwrap();
        let picked = open.lock().unwrap().files[&1].clone();
        let changed = slice(&picked, 0, 3).unwrap_err();
        assert_eq!(changed.message, "log.txt changed since it was picked");
    }

    #[test]
    fn a_pick_that_cannot_be_opened_is_refused_by_its_name_alone() {
        let dir = tempfile::tempdir().unwrap();
        let open = Mutex::default();
        let gone = opened(vec![dir.path().join("gone.txt")], &open).unwrap_err();
        assert_eq!(gone.code, refusal::HOST_FAULT);
        assert!(gone.message.starts_with("gone.txt: "), "{}", gone.message);
        let folder = opened(vec![dir.path().to_path_buf()], &open).unwrap_err();
        let where_it_is = dir.path().to_string_lossy();
        assert!(
            !folder.message.contains(&*where_it_is),
            "{}",
            folder.message
        );
    }
}
