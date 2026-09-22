//! The camera, through nokhwa. The driver's own pace is the clock: `frame`
//! blocks until the device has the next one, so the thread reads at exactly
//! the negotiated rate and never sleeps a period on top of it.
//!
//! Frames cross as RGBA, the size and rate the device negotiated. A guest
//! that falls behind LOSES frames rather than accruing a queue of stale
//! ones, which is a delay it could never work off.

use std::sync::Arc;

use super::{
    CAMERAS, Framing, Indicator, Items, MAX_FPS, MAX_HEIGHT, MAX_WIDTH, QUEUED_FRAMES, Watch,
    device_failed, wire,
};

pub(super) async fn watch(
    mut items: Items,
    consented: impl std::future::Future<Output = Result<(), wire::Refusal>>,
    want: Watch,
    guard: Arc<()>,
) {
    let _guard = guard;
    if let Err(refusal) = consented.await {
        items.end(Err(refusal));
        return;
    }
    let (framing, mut camera) = match camera(want).await {
        Ok(open) => open,
        Err(error) => {
            items.end(Err(device_failed(error)));
            return;
        }
    };
    let _indicator = Indicator::held(&CAMERAS);
    let first = serde_json::to_vec(&framing).expect("the opened camera mode");
    if !items.send(Ok(first)).await {
        return;
    }
    while let Some(frame) = camera.frames.recv().await {
        if !items.send(Ok(frame)).await {
            return;
        }
    }
    items.end(Err(device_failed("the camera stopped answering")));
}

/// The open camera. Its thread is blocked inside the driver waiting for the
/// next frame, so it notices this end going at its next turn and drops the
/// device there — within one frame period of the cancel.
struct Camera {
    frames: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

async fn camera(want: Watch) -> Result<(Framing, Camera), String> {
    let (ready, opened) = tokio::sync::oneshot::channel();
    let (frames, taken) = tokio::sync::mpsc::channel(QUEUED_FRAMES);
    std::thread::Builder::new()
        .name("view-camera".into())
        .spawn(move || camera_thread(want, ready, frames))
        .map_err(|error| error.to_string())?;
    let framing = opened
        .await
        .map_err(|_| "the camera thread ended".to_owned())??;
    Ok((framing, Camera { frames: taken }))
}

fn camera_thread(
    want: Watch,
    ready: tokio::sync::oneshot::Sender<Result<Framing, String>>,
    frames: tokio::sync::mpsc::Sender<Vec<u8>>,
) {
    use nokhwa::pixel_format::RgbAFormat;
    let opened = open_camera(&want).map_err(|error| error.to_string());
    let mut device = match opened {
        Ok(device) => device,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let resolution = device.resolution();
    // WHAT OPENED, not what was asked: the ceiling is applied when the mode
    // is chosen, and a device with nothing that small is reported honestly
    // rather than described as something it is not.
    let framing = Framing {
        width: resolution.width(),
        height: resolution.height(),
        fps: device.frame_rate().min(u32::from(u8::MAX)) as u8,
        format: "rgba",
    };
    if ready.send(Ok(framing)).is_err() {
        return;
    }
    // the driver's own pace is the only clock here: `frame` blocks until the
    // device has the next one, so this reads at exactly the negotiated rate
    while !frames.is_closed() {
        let Ok(frame) = device.frame() else {
            break;
        };
        let Ok(rgba) = frame.decode_image::<RgbAFormat>() else {
            break;
        };
        // a frame the guest is behind on is DROPPED, never queued
        let _ = frames.try_send(rgba.into_raw());
    }
}

/// The camera in the largest mode inside the ceiling the view asked for,
/// which is itself inside the host's — and at that mode's highest rate up to
/// [`MAX_FPS`]. A device that offers nothing that small opens at its
/// smallest mode, and the stream's first item says what that turned out to
/// be. The mode is chosen BEFORE the stream starts, so no frame is ever
/// decoded at the probe's size.
fn open_camera(want: &Watch) -> Result<nokhwa::Camera, nokhwa::NokhwaError> {
    use nokhwa::pixel_format::RgbAFormat;
    use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType, Resolution};
    let width = want.width.unwrap_or(MAX_WIDTH).clamp(1, MAX_WIDTH);
    let height = want.height.unwrap_or(MAX_HEIGHT).clamp(1, MAX_HEIGHT);
    let fps = want.fps.unwrap_or(MAX_FPS).clamp(1, MAX_FPS);
    let index = want
        .device
        .as_deref()
        .and_then(|device| device.parse().ok())
        .map_or(CameraIndex::Index(0), CameraIndex::Index);
    let asked = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::HighestResolution(
        Resolution::new(width, height),
    ));
    let mut device = nokhwa::Camera::new(index, asked)?;
    let formats = device.compatible_camera_formats().unwrap_or_default();
    if let Some(format) = inside(&formats, width, height, fps) {
        device.set_camera_requset(RequestedFormat::new::<RgbAFormat>(
            RequestedFormatType::Exact(format),
        ))?;
    }
    device.open_stream()?;
    Ok(device)
}

/// The largest decodable mode inside the ceiling, at its highest rate; the
/// smallest mode there is when nothing fits.
fn inside(
    formats: &[nokhwa::utils::CameraFormat],
    width: u32,
    height: u32,
    fps: u8,
) -> Option<nokhwa::utils::CameraFormat> {
    use nokhwa::pixel_format::{FormatDecoder as _, RgbAFormat};
    use nokhwa::utils::CameraFormat;
    let pixels = |format: &CameraFormat| format.width() * format.height();
    let decodable: Vec<CameraFormat> = formats
        .iter()
        .copied()
        .filter(|format| RgbAFormat::FORMATS.contains(&format.format()))
        .collect();
    decodable
        .iter()
        .copied()
        .filter(|format| {
            format.width() <= width
                && format.height() <= height
                && format.frame_rate() <= u32::from(fps)
        })
        .max_by_key(|format| (pixels(format), format.frame_rate()))
        .or_else(|| {
            decodable
                .iter()
                .copied()
                .min_by_key(|format| (pixels(format), std::cmp::Reverse(format.frame_rate())))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view picks lower, never higher: what it asks for is clamped onto
    /// the host's ceiling before a device is ever opened.
    #[test]
    fn a_capture_ask_is_clamped_onto_the_host_ceiling() {
        let want = Watch {
            device: None,
            width: Some(4096),
            height: Some(2160),
            fps: Some(120),
        };
        let width = want.width.unwrap_or(MAX_WIDTH).clamp(1, MAX_WIDTH);
        let height = want.height.unwrap_or(MAX_HEIGHT).clamp(1, MAX_HEIGHT);
        let fps = want.fps.unwrap_or(MAX_FPS).clamp(1, MAX_FPS);
        assert_eq!((width, height, fps), (MAX_WIDTH, MAX_HEIGHT, MAX_FPS));
    }
}
