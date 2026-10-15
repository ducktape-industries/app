//! Linux: cameras through V4L2, our own calls. The cameras are the video
//! nodes sysfs lists that can capture; one opened camera is a stream of
//! four mapped buffers that [`Camera::next`] waits on with `poll` and a
//! time limit, so a camera that stops or goes away is seen (`ENODEV`,
//! `EIO`, or the stream loop's no-frame deadline). Dropping a [`Camera`]
//! stops the stream, unmaps the buffers and closes the node: the device
//! is free at once (`modprobe -r` of its driver succeeds).
//!
//! The few V4L2 structs are our own, checked against `linux/videodev2.h`
//! by size (the ioctl numbers below carry it): no bindgen, no libclang.
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::Duration;

use crate::backend::{Device, Got, Picture, Source, Status};

const YUYV: u32 = u32::from_le_bytes(*b"YUYV");
const MJPG: u32 = u32::from_le_bytes(*b"MJPG");

/// What `choose` asks a camera for first: a picture this size, this often.
const WANT: (u32, u32, u32) = (1280, 720, 30);

// ---------- the V4L2 calls (linux/videodev2.h, 64-bit) ----------

// the numbers carry each struct's size; the asserts below hold the structs to it
const VIDIOC_QUERYCAP: u64 = 0x8068_5600;
const VIDIOC_S_FMT: u64 = 0xc0d0_5605;
const VIDIOC_REQBUFS: u64 = 0xc014_5608;
const VIDIOC_QUERYBUF: u64 = 0xc058_5609;
const VIDIOC_QBUF: u64 = 0xc058_560f;
const VIDIOC_DQBUF: u64 = 0xc058_5611;
const VIDIOC_STREAMON: u64 = 0x4004_5612;
const VIDIOC_STREAMOFF: u64 = 0x4004_5613;
const VIDIOC_S_PARM: u64 = 0xc0cc_5616;

const CAP_VIDEO_CAPTURE: u32 = 0x1;
const CAP_STREAMING: u32 = 0x0400_0000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const MEMORY_MMAP: u32 = 1;
const BUF_FLAG_ERROR: u32 = 0x40;
const BUFFERS: u32 = 4;

/// `struct v4l2_capability`
#[repr(C)]
struct Capability {
    _driver_card_bus: [u8; 80],
    _version: u32,
    capabilities: u32,
    device_caps: u32,
    _reserved: [u32; 3],
}

/// `struct v4l2_format` holding a `struct v4l2_pix_format`: the union
/// starts at 8 (it holds pointers) and is 200 bytes.
#[repr(C)]
struct Format {
    kind: u32,
    _align: u32,
    width: u32,
    height: u32,
    pixelformat: u32,
    _field: u32,
    bytesperline: u32,
    _pix: [u32; 7],
    _union: [u8; 152],
}

/// `struct v4l2_streamparm` holding a `struct v4l2_captureparm`.
#[repr(C)]
struct StreamParm {
    kind: u32,
    _capability_mode: [u32; 2],
    /// `timeperframe`: seconds per frame, as a fraction
    numerator: u32,
    denominator: u32,
    _union: [u32; 46],
}

/// `struct v4l2_requestbuffers`
#[repr(C)]
struct RequestBuffers {
    count: u32,
    kind: u32,
    memory: u32,
    _capabilities_flags: [u32; 2],
}

/// `struct v4l2_buffer`, single-planar, mmap'd.
#[repr(C)]
struct Buffer {
    index: u32,
    kind: u32,
    bytesused: u32,
    flags: u32,
    /// `field`, an alignment hole, `timestamp`, `timecode`, `sequence`
    _between: [u32; 11],
    memory: u32,
    /// `m.offset`; the union is 8 bytes
    offset: u32,
    _m: u32,
    length: u32,
    /// `reserved2`, `request_fd`, the end's alignment
    _end: [u32; 3],
}

const _: () = assert!(size_of::<Capability>() == 104);
const _: () = assert!(size_of::<Format>() == 208);
const _: () = assert!(size_of::<StreamParm>() == 204);
const _: () = assert!(size_of::<RequestBuffers>() == 20);
const _: () = assert!(size_of::<Buffer>() == 88);
// and the fields read, where the header puts them
const _: () = assert!(std::mem::offset_of!(Capability, device_caps) == 88);
const _: () = assert!(std::mem::offset_of!(Format, width) == 8);
const _: () = assert!(std::mem::offset_of!(Format, bytesperline) == 24);
const _: () = assert!(std::mem::offset_of!(StreamParm, numerator) == 12);
const _: () = assert!(std::mem::offset_of!(Buffer, memory) == 60);
const _: () = assert!(std::mem::offset_of!(Buffer, offset) == 64);
const _: () = assert!(std::mem::offset_of!(Buffer, length) == 72);

fn zeroed<T>() -> T {
    // SAFETY: only the plain integer structs above, all-zero valid
    unsafe { std::mem::zeroed() }
}

/// One ioctl; `Err` is its errno. Interrupted calls are made again.
fn ioctl<T>(fd: &OwnedFd, request: u64, arg: &mut T) -> Result<(), i32> {
    loop {
        // SAFETY: `request` is one of the numbers above, each made for `T`
        if unsafe { libc::ioctl(fd.as_raw_fd(), request as _, arg as *mut T) } >= 0 {
            return Ok(());
        }
        match errno() {
            libc::EINTR => continue,
            error => return Err(error),
        }
    }
}

fn errno() -> i32 {
    io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn open_node(path: &Path) -> Result<OwnedFd, i32> {
    let path = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| libc::ENOENT)?;
    // non-blocking: a frame is waited for in `poll`, with a time limit
    // SAFETY: a valid C string; the descriptor is owned from here
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    match fd {
        -1 => Err(errno()),
        // SAFETY: a fresh descriptor, owned by nothing else
        fd => Ok(unsafe { OwnedFd::from_raw_fd(fd) }),
    }
}

/// What an errno says, as a status: `streaming`, the camera was
/// delivering (a vanished device is gone, not missing).
pub(crate) fn status(errno: i32, streaming: bool) -> Status {
    match errno {
        libc::EBUSY => Status::Busy,
        libc::EACCES | libc::EPERM => Status::Denied,
        libc::ENODEV | libc::ENXIO | libc::EIO if streaming => Status::Gone,
        libc::ENODEV | libc::ENXIO | libc::ENOENT => Status::NoDevice,
        _ => Status::Failed(io::Error::from_raw_os_error(errno).to_string()),
    }
}

// ---------- the cameras ----------

/// One video node as listed: its node (`video0`), its stable id, its
/// name, the device it hangs off, and what it can do (an errno when it
/// could not be opened).
#[derive(Debug)]
pub(crate) struct Node {
    pub node: String,
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub caps: Result<u32, i32>,
}

/// The cameras the system lists now; the first is the default.
pub(crate) fn cameras() -> Result<Vec<Device>, String> {
    Ok(listed(nodes().map_err(|error| error.to_string())?))
}

/// Every video node sysfs lists, by number, with what it can do.
fn nodes() -> io::Result<Vec<Node>> {
    let entries = match fs::read_dir("/sys/class/video4linux") {
        Ok(entries) => entries,
        // no V4L2 driver loaded: no cameras
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found: Vec<(u32, String)> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some((name.strip_prefix("video")?.parse().ok()?, name))
        })
        .collect();
    found.sort();
    Ok(found
        .into_iter()
        .map(|(_, node)| {
            let sys = Path::new("/sys/class/video4linux").join(&node);
            let read = |what: &str| fs::read_to_string(sys.join(what)).unwrap_or_default();
            let (id, parent) = identity(&node, &sys, read("index").trim());
            let caps = open_node(&Path::new("/dev").join(&node)).and_then(|fd| {
                let mut cap: Capability = zeroed();
                ioctl(&fd, VIDIOC_QUERYCAP, &mut cap)?;
                Ok(match cap.capabilities & CAP_DEVICE_CAPS {
                    0 => cap.capabilities,
                    _ => cap.device_caps,
                })
            });
            Node {
                node,
                id,
                name: read("name").trim().to_owned(),
                parent,
                caps,
            }
        })
        .collect())
}

/// A node's stable id, udev's `by-path` scheme: the device it hangs off
/// (a USB camera's interface, the same at the same port across replugs)
/// and its index there; a node with no device (a virtual one) by its
/// own name. Two cameras in one USB device (colour and infrared) hang off
/// two interfaces, so they differ.
fn identity(node: &str, sys: &Path, index: &str) -> (String, Option<String>) {
    match fs::canonicalize(sys.join("device")) {
        Ok(parent) => {
            let parent = parent.to_string_lossy().into_owned();
            (format!("{parent}#{index}"), Some(parent))
        }
        Err(_) => (node.to_owned(), None),
    }
}

/// The nodes a person can pick: one row a camera. A node that captures
/// and streams is a camera; the others (a camera's metadata node, an
/// output) are not. A node that could not be opened is listed by its
/// name (opening it says why: refused, busy), once per device it hangs
/// off, as its metadata twin could not be opened either.
pub(crate) fn listed(nodes: Vec<Node>) -> Vec<Device> {
    let mut unread = std::collections::HashSet::new();
    nodes
        .into_iter()
        .filter(|node| match node.caps {
            Ok(caps) => {
                caps & (CAP_VIDEO_CAPTURE | CAP_STREAMING) == CAP_VIDEO_CAPTURE | CAP_STREAMING
            }
            Err(_) => node
                .parent
                .as_ref()
                .is_none_or(|parent| unread.insert(parent.clone())),
        })
        .enumerate()
        .map(|(at, node)| Device {
            name: match node.name.is_empty() {
                true => node.id.clone(),
                false => node.name,
            },
            id: node.id,
            default: at == 0,
        })
        .collect()
}

// ---------- one camera ----------

/// A picture mode a camera agreed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Mode {
    pub fourcc: u32,
    pub width: u32,
    pub height: u32,
    /// bytes per row
    pub stride: u32,
    /// frames a second, where the driver says
    pub fps: Option<u32>,
}

/// The mode to stream, of what the camera agreed to when asked for each
/// format at [`WANT`]: a smooth rate first (a USB 2 webcam sends YUYV at
/// 720p only about 10 times a second, MJPEG at 30), then the larger
/// picture up to 720p, then YUYV (no decoding).
pub(crate) fn choose(agreed: &[Mode]) -> Option<Mode> {
    let (width, height, _) = WANT;
    agreed
        .iter()
        .filter(|mode| matches!(mode.fourcc, YUYV | MJPG) && mode.width > 0 && mode.height > 0)
        .max_by_key(|mode| {
            (
                mode.fps.is_none_or(|fps| fps >= 24),
                (mode.width * mode.height).min(width * height),
                mode.fourcc == YUYV,
            )
        })
        .copied()
}

/// Asks the camera for `fourcc` at [`WANT`]'s size and rate; what it
/// agreed to. `S_FMT` and `S_PARM` answer with what the driver took.
fn agree(fd: &OwnedFd, fourcc: u32, width: u32, height: u32) -> Result<Mode, i32> {
    let mut format: Format = zeroed();
    format.kind = BUF_TYPE_VIDEO_CAPTURE;
    format.width = width;
    format.height = height;
    format.pixelformat = fourcc;
    ioctl(fd, VIDIOC_S_FMT, &mut format)?;
    let mut parm: StreamParm = zeroed();
    parm.kind = BUF_TYPE_VIDEO_CAPTURE;
    parm.numerator = 1;
    parm.denominator = WANT.2;
    // a driver without frame rates says nothing of its rate
    let fps = ioctl(fd, VIDIOC_S_PARM, &mut parm)
        .ok()
        .filter(|()| parm.numerator > 0)
        .map(|()| parm.denominator / parm.numerator);
    Ok(Mode {
        fourcc: format.pixelformat,
        width: format.width,
        height: format.height,
        stride: format.bytesperline,
        fps,
    })
}

/// One mapped buffer, unmapped when dropped.
struct Mapped {
    at: *mut libc::c_void,
    length: usize,
}

impl Drop for Mapped {
    fn drop(&mut self) {
        // SAFETY: mapped in `open` with this length, unmapped once
        unsafe { libc::munmap(self.at, self.length) };
    }
}

/// One camera streaming: its node and its mapped buffers. Dropped, the
/// stream stops, the buffers are unmapped and the node is closed, in
/// that order (a mapping holds the device as an open node does).
pub(crate) struct Camera {
    mode: Mode,
    buffers: Vec<Mapped>,
    fd: OwnedFd,
}

impl Drop for Camera {
    fn drop(&mut self) {
        let mut kind = BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
        let _ = ioctl(&self.fd, VIDIOC_STREAMOFF, &mut kind);
        // then the fields: `buffers` unmapped, then `fd` closed
    }
}

/// Opens the camera `id` names and starts its stream.
pub(crate) fn open(id: &str) -> Result<Camera, Status> {
    let node = nodes()
        .map_err(|error| Status::Failed(error.to_string()))?
        .into_iter()
        .find(|node| node.id == id)
        .ok_or(Status::NoDevice)?;
    let fail = |errno| status(errno, false);
    let fd = open_node(&Path::new("/dev").join(node.node)).map_err(fail)?;
    let mut agreed = Vec::new();
    for fourcc in [YUYV, MJPG] {
        match agree(&fd, fourcc, WANT.0, WANT.1) {
            Ok(mode) => agreed.push(mode),
            // another app streams from it
            Err(libc::EBUSY) => return Err(Status::Busy),
            Err(_) => {}
        }
    }
    let mode = choose(&agreed)
        .ok_or_else(|| Status::Failed("the camera sends neither YUYV nor MJPEG pictures".into()))?;
    // the camera holds the last mode asked; the chosen one again if not it
    let mode = match agreed.last() == Some(&mode) {
        true => mode,
        false => agree(&fd, mode.fourcc, mode.width, mode.height).map_err(fail)?,
    };
    let mut request = RequestBuffers {
        count: BUFFERS,
        kind: BUF_TYPE_VIDEO_CAPTURE,
        memory: MEMORY_MMAP,
        _capabilities_flags: [0; 2],
    };
    ioctl(&fd, VIDIOC_REQBUFS, &mut request).map_err(fail)?;
    if request.count == 0 {
        return Err(Status::Failed("the camera gave no buffers".into()));
    }
    let mut buffers = Vec::new();
    for index in 0..request.count {
        let mut buffer = capture_buffer(index);
        ioctl(&fd, VIDIOC_QUERYBUF, &mut buffer).map_err(fail)?;
        let length = buffer.length as usize;
        // SAFETY: the driver's offset and length for this buffer
        let at = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                buffer.offset as libc::off_t,
            )
        };
        if at == libc::MAP_FAILED {
            return Err(fail(errno()));
        }
        buffers.push(Mapped { at, length });
        ioctl(&fd, VIDIOC_QBUF, &mut buffer).map_err(fail)?;
    }
    let mut kind = BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
    ioctl(&fd, VIDIOC_STREAMON, &mut kind).map_err(fail)?;
    Ok(Camera { mode, buffers, fd })
}

fn capture_buffer(index: u32) -> Buffer {
    let mut buffer: Buffer = zeroed();
    buffer.index = index;
    buffer.kind = BUF_TYPE_VIDEO_CAPTURE;
    buffer.memory = MEMORY_MMAP;
    buffer
}

impl Source for Camera {
    fn next(&mut self, wait: Duration, wanted: bool, mirror: bool) -> Result<Got, Status> {
        let mut ready = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = wait.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one valid pollfd
        match unsafe { libc::poll(&mut ready, 1, millis) } {
            0 => return Ok(Got::Nothing),
            -1 if errno() == libc::EINTR => return Ok(Got::Nothing),
            -1 => return Err(status(errno(), true)),
            // readable, or an error (POLLERR) the dequeue reports
            _ => {}
        }
        let mut buffer = capture_buffer(0);
        match ioctl(&self.fd, VIDIOC_DQBUF, &mut buffer) {
            Ok(()) => {}
            Err(libc::EAGAIN) => return Ok(Got::Nothing),
            Err(errno) => return Err(status(errno, true)),
        }
        let got = match self.buffers.get(buffer.index as usize) {
            // a frame the driver marked broken is no frame
            _ if buffer.flags & BUF_FLAG_ERROR != 0 => Got::Nothing,
            Some(mapped) if wanted => {
                let used = match buffer.bytesused {
                    0 => mapped.length,
                    used => (used as usize).min(mapped.length),
                };
                // SAFETY: the mapping lives as long as `self`; the driver
                // writes it again only once it is queued back, below
                let data = unsafe { std::slice::from_raw_parts(mapped.at as *const u8, used) };
                match self.picture(data, mirror) {
                    Some(picture) => Got::Picture(picture),
                    None => Got::Frame,
                }
            }
            _ => Got::Frame,
        };
        ioctl(&self.fd, VIDIOC_QBUF, &mut buffer).map_err(|errno| status(errno, true))?;
        Ok(got)
    }
}

impl Camera {
    fn picture(&self, data: &[u8], mirror: bool) -> Option<Picture> {
        let Mode {
            fourcc,
            width,
            height,
            stride,
            ..
        } = self.mode;
        match fourcc {
            YUYV => yuyv_to_bgra(data, width, height, stride, mirror),
            _ => jpeg_to_bgra(data, mirror),
        }
    }
}

// ---------- pictures ----------

/// Packed 4:2:2 (Y0 U Y1 V, BT.601 video range) to BGRA, rows `stride`
/// bytes apart (0: unpadded).
pub(crate) fn yuyv_to_bgra(
    src: &[u8],
    width: u32,
    height: u32,
    stride: u32,
    mirror: bool,
) -> Option<Picture> {
    let (w, h) = (width as usize, height as usize);
    let stride = (stride as usize).max(w * 2);
    if w < 2 || h == 0 || src.len() < stride * (h - 1) + w * 2 {
        return None;
    }
    let mut bgra = vec![0u8; w * h * 4];
    for (y, out) in bgra.chunks_exact_mut(w * 4).enumerate() {
        let row = &src[y * stride..y * stride + w * 2];
        for x in 0..w {
            let pair = &row[(x & !1) * 2..(x & !1) * 2 + 4];
            let luma = pair[if x & 1 == 0 { 0 } else { 2 }];
            let (b, g, r) = yuv(luma, pair[1], pair[3]);
            let at = if mirror { w - 1 - x } else { x } * 4;
            out[at..at + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }
    Some(Picture {
        width,
        height,
        bgra,
    })
}

fn yuv(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = (i32::from(y) - 16) * 298;
    let d = i32::from(u) - 128;
    let e = i32::from(v) - 128;
    let clamp = |value: i32| ((value + 128) >> 8).clamp(0, 255) as u8;
    (
        clamp(c + 516 * d),
        clamp(c - 100 * d - 208 * e),
        clamp(c + 409 * e),
    )
}

/// A MJPEG frame (one JPEG) to BGRA; none for a frame that does not
/// decode.
pub(crate) fn jpeg_to_bgra(src: &[u8], mirror: bool) -> Option<Picture> {
    let decoded = image::load_from_memory_with_format(src, image::ImageFormat::Jpeg).ok()?;
    let mut rgba = decoded.to_rgba8();
    if mirror {
        image::imageops::flip_horizontal_in_place(&mut rgba);
    }
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(Picture {
        width,
        height,
        bgra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(fourcc: u32, width: u32, height: u32, fps: Option<u32>) -> Mode {
        Mode {
            fourcc,
            width,
            height,
            stride: width * 2,
            fps,
        }
    }

    /// A USB 2 webcam streams MJPEG at 720p30 over YUYV at 720p10; a
    /// camera that sends YUYV smoothly keeps YUYV; a small YUYV-only
    /// camera is taken as it is; a format we cannot draw is not.
    #[test]
    fn the_mode_is_smooth_then_large_then_yuyv() {
        let webcam = [
            mode(YUYV, 1280, 720, Some(10)),
            mode(MJPG, 1280, 720, Some(30)),
        ];
        assert_eq!(choose(&webcam), Some(webcam[1]));
        let loopback = [
            mode(YUYV, 1280, 720, Some(30)),
            mode(YUYV, 1280, 720, Some(30)),
        ];
        assert_eq!(choose(&loopback).map(|m| m.fourcc), Some(YUYV));
        let both = [
            mode(YUYV, 1280, 720, Some(30)),
            mode(MJPG, 1920, 1080, Some(30)),
        ];
        assert_eq!(choose(&both), Some(both[0]));
        let old = [mode(YUYV, 640, 480, Some(30))];
        assert_eq!(choose(&old), Some(old[0]));
        let silent = [mode(YUYV, 640, 480, None), mode(MJPG, 1280, 720, None)];
        assert_eq!(choose(&silent), Some(silent[1]));
        let grey = u32::from_le_bytes(*b"GREY");
        assert_eq!(choose(&[mode(grey, 1280, 720, Some(30))]), None);
        assert_eq!(choose(&[]), None);
    }

    /// A UVC camera's capture and metadata nodes come down to its capture
    /// node; a loopback with no writer (output only) is no camera; nodes
    /// that could not be opened are listed once per device they hang off.
    #[test]
    fn video_nodes_come_down_to_one_row_a_camera() {
        let node = |id: &str, name: &str, parent: Option<&str>, caps: Result<u32, i32>| Node {
            node: String::new(),
            id: id.into(),
            name: name.into(),
            parent: parent.map(Into::into),
            caps,
        };
        let capture = CAP_VIDEO_CAPTURE | CAP_STREAMING;
        let meta = 0x0080_0000 | CAP_STREAMING;
        let listed = listed(vec![
            node("usb-1#0", "Integrated Camera", Some("usb-1"), Ok(capture)),
            node("usb-1#1", "Integrated Camera", Some("usb-1"), Ok(meta)),
            node("usb-2#0", "IR Camera", Some("usb-2"), Ok(capture)),
            node(
                "video40",
                "Dummy video device",
                None,
                Ok(0x2 | CAP_STREAMING),
            ),
            node("usb-3#0", "Locked Camera", Some("usb-3"), Err(libc::EACCES)),
            node("usb-3#1", "Locked Camera", Some("usb-3"), Err(libc::EACCES)),
            node("video42", "Test Camera", None, Err(libc::EBUSY)),
            node("video43", "", None, Ok(capture)),
        ]);
        let rows: Vec<_> = listed
            .iter()
            .map(|d| (d.id.as_str(), d.name.as_str(), d.default))
            .collect();
        assert_eq!(
            rows,
            [
                ("usb-1#0", "Integrated Camera", true),
                ("usb-2#0", "IR Camera", false),
                ("usb-3#0", "Locked Camera", false),
                ("video42", "Test Camera", false),
                ("video43", "video43", false),
            ]
        );
    }

    /// The errnos a camera answers with, as the states a person sees.
    #[test]
    fn an_errno_reads_as_a_status() {
        assert_eq!(status(libc::EBUSY, false), Status::Busy);
        assert_eq!(status(libc::EACCES, false), Status::Denied);
        assert_eq!(status(libc::ENOENT, false), Status::NoDevice);
        assert_eq!(status(libc::ENODEV, false), Status::NoDevice);
        assert_eq!(status(libc::ENODEV, true), Status::Gone);
        assert_eq!(status(libc::EIO, true), Status::Gone);
        assert!(matches!(status(libc::EINVAL, false), Status::Failed(_)));
    }

    /// Grey, white and a mirrored pair come out of 4:2:2 as BGRA; rows
    /// are read by the driver's stride.
    #[test]
    fn yuyv_turns_into_bgra_and_mirrors() {
        // two pixels: black (Y 16) then white (Y 235), no colour
        let src = [16, 128, 235, 128];
        let plain = yuyv_to_bgra(&src, 2, 1, 0, false).unwrap();
        assert_eq!(plain.bgra, [0, 0, 0, 255, 255, 255, 255, 255]);
        let mirrored = yuyv_to_bgra(&src, 2, 1, 4, true).unwrap();
        assert_eq!(mirrored.bgra, [255, 255, 255, 255, 0, 0, 0, 255]);
        // pure red in BT.601 video range: Y 81, U 90, V 240
        let red = yuyv_to_bgra(&[81, 90, 81, 240], 2, 1, 4, false).unwrap();
        let [b, g, r, _] = red.bgra[..4] else {
            unreachable!()
        };
        assert!(r > 250 && g < 5 && b < 5, "{:?}", &red.bgra[..4]);
        // a short buffer is no picture; a padded row is read by its stride
        assert!(yuyv_to_bgra(&src[..3], 2, 1, 4, false).is_none());
        let padded = [16, 128, 235, 128, 9, 9, 235, 128, 16, 128];
        let two = yuyv_to_bgra(&padded, 2, 2, 6, false).unwrap();
        assert_eq!(two.bgra[8..], [255, 255, 255, 255, 0, 0, 0, 255]);
    }

    /// A MJPEG frame decodes to BGRA, mirrored if asked; a broken one is
    /// no picture.
    #[test]
    fn a_jpeg_frame_turns_into_bgra() {
        let mut rgb = image::RgbImage::new(16, 8);
        for (x, _, pixel) in rgb.enumerate_pixels_mut() {
            *pixel = if x < 8 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            };
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&rgb)
            .unwrap();
        let left = |picture: &Picture| {
            let [b, g, r, a] = picture.bgra[..4] else {
                unreachable!()
            };
            (b, g, r, a)
        };
        let plain = jpeg_to_bgra(&jpeg, false).unwrap();
        assert_eq!((plain.width, plain.height), (16, 8));
        let (b, _, r, a) = left(&plain);
        assert!(r > 200 && b < 60 && a == 255, "{:?}", left(&plain));
        let (b, _, r, _) = left(&jpeg_to_bgra(&jpeg, true).unwrap());
        assert!(b > 200 && r < 60);
        assert!(jpeg_to_bgra(&jpeg[..20], false).is_none());
    }
}
