//! The microphone and the output device, both cpal. A cpal stream is not
//! `Send`, so each one lives on a thread of its own and the guest side never
//! holds more than a channel end: dropping that end is what releases the
//! device, whatever dropped it.
//!
//! Nothing here transforms a sample. Captured audio crosses as the driver
//! produced it — interleaved i16, little-endian — and written audio reaches
//! the device the same way. A view that wants a rate the device does not
//! offer resamples in its own wasm.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::{
    CHUNK_MS, Guest, Indicator, Items, Listen, MICROPHONES, Media, Opened, Out, PLAYOUT_MS,
    Playout, QUEUED_CHUNKS, Speak, device_failed, spawn_device, wire,
};

pub(super) async fn listen(
    mut items: Items,
    consented: impl std::future::Future<Output = Result<(), wire::Refusal>>,
    want: Listen,
    guard: Arc<()>,
) {
    let _guard = guard;
    if let Err(refusal) = consented.await {
        items.end(Err(refusal));
        return;
    }
    let (opened, mut chunks) = match microphone(want).await {
        Ok(open) => open,
        Err(error) => {
            items.end(Err(device_failed(error)));
            return;
        }
    };
    let _indicator = Indicator::held(&MICROPHONES);
    let first = serde_json::to_vec(&opened).expect("the opened audio mode");
    if !items.send(Ok(first)).await {
        return;
    }
    while let Some(chunk) = chunks.samples.recv().await {
        if !items.send(Ok(chunk)).await {
            return;
        }
    }
    items.end(Err(device_failed("the microphone stopped answering")));
}

/// The open microphone: dropping this ends its thread, which drops the cpal
/// stream — a cpal stream is not `Send`, so it lives on a thread of its own
/// and never crosses back.
struct Microphone {
    samples: tokio::sync::mpsc::Receiver<Vec<u8>>,
    _stop: std::sync::mpsc::Sender<()>,
}

/// Opening waits on the device, never on this runtime: a microphone whose
/// system prompt is unanswered parks its opener for as long as the prompt
/// is up, and this runtime is the one every view's replies come through.
async fn microphone(want: Listen) -> Result<(Opened, Microphone), String> {
    let (ready, opened) = tokio::sync::oneshot::channel();
    let (samples, taken) = tokio::sync::mpsc::channel(QUEUED_CHUNKS);
    let (stop, stopped) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("view-microphone".into())
        .spawn(move || microphone_thread(want, ready, samples, stopped))
        .map_err(|error| error.to_string())?;
    let opened = opened
        .await
        .map_err(|_| "the microphone thread ended".to_owned())??;
    Ok((
        opened,
        Microphone {
            samples: taken,
            _stop: stop,
        },
    ))
}

fn microphone_thread(
    want: Listen,
    ready: tokio::sync::oneshot::Sender<Result<Opened, String>>,
    samples: tokio::sync::mpsc::Sender<Vec<u8>>,
    stopped: std::sync::mpsc::Receiver<()>,
) {
    use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
    let opened = (|| {
        let host = cpal::default_host();
        let device = match &want.device {
            Some(named) => host
                .input_devices()
                .map_err(|error| error.to_string())?
                .find(|device| device.name().is_ok_and(|name| &name == named))
                .ok_or_else(|| "this host has no such input device".to_owned())?,
            None => host
                .default_input_device()
                .ok_or_else(|| "this host has no microphone".to_owned())?,
        };
        let config = input_config(&device, &want)?;
        let opened = Opened {
            rate: config.sample_rate().0,
            channels: config.channels() as u8,
        };
        let chunk = chunk_samples(opened);
        let format = config.sample_format();
        let config = config.into();
        let fault = |_| {};
        let stream = match format {
            cpal::SampleFormat::I16 => {
                let mut buffer = Vec::with_capacity(chunk);
                let samples = samples.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _| pump(&mut buffer, data.iter().copied(), chunk, &samples),
                    fault,
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let mut buffer = Vec::with_capacity(chunk);
                let samples = samples.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _| {
                        pump(
                            &mut buffer,
                            data.iter().copied().map(whole),
                            chunk,
                            &samples,
                        )
                    },
                    fault,
                    None,
                )
            }
            format => return Err(format!("this microphone speaks {format}")),
        };
        let stream = stream.map_err(|error| error.to_string())?;
        stream.play().map_err(|error| error.to_string())?;
        Ok((stream, opened))
    })();
    match opened {
        Ok((stream, opened)) => {
            let _ = ready.send(Ok(opened));
            // the sender goes with the subscription: that is the release
            let _ = stopped.recv();
            drop(stream);
        }
        Err(error) => {
            let _ = ready.send(Err(error));
        }
    }
}

/// How many samples one item carries: whole sample ticks across the
/// channels, at [`CHUNK_MS`].
fn chunk_samples(opened: Opened) -> usize {
    let ticks = (opened.rate as usize * CHUNK_MS as usize).div_ceil(1000);
    ticks.max(1) * opened.channels.max(1) as usize
}

/// Device callback to item bytes: interleaved i16, little-endian, exactly
/// as captured. A chunk the guest is too far behind to take is DROPPED
/// here — the alternative is a queue that grows without bound inside a
/// realtime callback.
fn pump(
    buffer: &mut Vec<i16>,
    samples: impl Iterator<Item = i16>,
    chunk: usize,
    out: &tokio::sync::mpsc::Sender<Vec<u8>>,
) {
    buffer.extend(samples);
    while buffer.len() >= chunk {
        let bytes = buffer
            .drain(..chunk)
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<u8>>();
        let _ = out.try_send(bytes);
    }
}

/// A float sample as a whole one, clamped.
fn whole(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// The mode the view asked for WHEN THE DEVICE OFFERS IT, the device's own
/// default otherwise. No resampling: the host hands over what the driver
/// produced and says so in the stream's first item.
fn input_config(
    device: &cpal::Device,
    want: &Listen,
) -> Result<cpal::SupportedStreamConfig, String> {
    use cpal::traits::DeviceTrait as _;
    let offered = want.rate.and_then(|rate| {
        device
            .supported_input_configs()
            .ok()
            .into_iter()
            .flatten()
            .find(|range| {
                readable(range.sample_format())
                    && want
                        .channels
                        .is_none_or(|ask| range.channels() == u16::from(ask))
                    && (range.min_sample_rate().0..=range.max_sample_rate().0).contains(&rate)
            })
            .map(|range| range.with_sample_rate(cpal::SampleRate(rate)))
    });
    match offered {
        Some(config) => Ok(config),
        None => device
            .default_input_config()
            .map_err(|error| error.to_string()),
    }
}

fn readable(format: cpal::SampleFormat) -> bool {
    matches!(format, cpal::SampleFormat::I16 | cpal::SampleFormat::F32)
}

// ---------- audio playout ----------

pub(super) fn play(guest: &mut Guest, id: u64, payload: &[u8]) {
    let want = match serde_json::from_slice::<Speak>(payload) {
        Ok(want) if (8_000..=192_000).contains(&want.rate) && (1..=8).contains(&want.channels) => {
            want
        }
        Ok(_) => {
            guest.refuse(
                id,
                "malformed_request",
                "`audio.play` names no playable mode",
            );
            return;
        }
        Err(error) => {
            guest.refuse(id, "malformed_request", error.to_string());
            return;
        }
    };
    if guest.media.out.as_ref().is_some_and(open) {
        guest.refuse(id, "device_busy", "this view already opened an output");
        return;
    }
    let playout = Arc::new(Mutex::new(Playout {
        samples: VecDeque::new(),
        ceiling: ceiling(want.rate, want.channels),
        open: true,
    }));
    let (ready, opened) = tokio::sync::oneshot::channel();
    let (stop, stopped) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("view-output".into())
        .spawn({
            let playout = playout.clone();
            move || output_thread(want, playout, ready, stopped)
        });
    if let Err(error) = thread {
        guest.refuse(id, "device_failed", error.to_string());
        return;
    }
    guest.media.out = Some(Out {
        playout,
        _stop: stop,
    });
    spawn_device(guest, id, async move {
        let opened = opened
            .await
            .map_err(|_| device_failed("the output thread ended"))?
            .map_err(device_failed)?;
        serde_json::to_vec(&serde_json::json!({
            "handle": "out", "rate": opened.rate, "channels": opened.channels
        }))
        .map_err(device_failed)
    });
}

fn open(out: &Out) -> bool {
    out.playout.lock().expect("playout").open
}

/// The unplayed ceiling: [`PLAYOUT_MS`] of the mode that opened.
fn ceiling(rate: u32, channels: u8) -> usize {
    (rate as usize * channels.max(1) as usize * PLAYOUT_MS as usize) / 1000
}

/// One `audio.write`: interleaved i16 little-endian, appended to the queue
/// the device drains. Refused rather than queued past the ceiling, so a
/// view that is producing faster than the device plays is TOLD.
pub(super) fn written(media: &Media, payload: &[u8]) -> Result<(), wire::Refusal> {
    let Some(out) = media.out.as_ref() else {
        return Err(wire::Refusal::new(
            "not_open",
            "`audio.write` before `audio.play`",
        ));
    };
    if !payload.len().is_multiple_of(2) {
        return Err(wire::Refusal::new(
            "malformed_request",
            "`audio.write` takes whole interleaved i16 samples",
        ));
    }
    let mut playout = out.playout.lock().expect("playout");
    if !playout.open {
        return Err(wire::Refusal::new(
            "not_open",
            "the output device is not open",
        ));
    }
    if playout.samples.len() + payload.len() / 2 > playout.ceiling {
        return Err(wire::Refusal::new(
            "backpressure",
            "the output queue is full; write again once it drains",
        ));
    }
    playout.samples.extend(
        payload
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]])),
    );
    Ok(())
}

fn output_thread(
    want: Speak,
    playout: Arc<Mutex<Playout>>,
    ready: tokio::sync::oneshot::Sender<Result<Opened, String>>,
    stopped: std::sync::mpsc::Receiver<()>,
) {
    use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
    let opened = (|| {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "this host has no output device".to_owned())?;
        let config = output_config(&device, &want)?;
        let opened = Opened {
            rate: config.sample_rate().0,
            channels: config.channels() as u8,
        };
        let format = config.sample_format();
        let config = config.into();
        let fault = |_| {};
        let stream = match format {
            cpal::SampleFormat::I16 => {
                let playout = playout.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| drain(&playout, data, |sample| sample),
                    fault,
                    None,
                )
            }
            cpal::SampleFormat::F32 => {
                let playout = playout.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        drain(&playout, data, |sample| f32::from(sample) / 32768.0)
                    },
                    fault,
                    None,
                )
            }
            format => return Err(format!("this output device speaks {format}")),
        };
        let stream = stream.map_err(|error| error.to_string())?;
        stream.play().map_err(|error| error.to_string())?;
        Ok((stream, opened))
    })();
    match opened {
        Ok((stream, opened)) => {
            playout.lock().expect("playout").ceiling = ceiling(opened.rate, opened.channels);
            let _ = ready.send(Ok(opened));
            let _ = stopped.recv();
            drop(stream);
        }
        Err(error) => {
            let _ = ready.send(Err(error));
        }
    }
    // whatever ended this, a write after it is refused rather than swallowed
    playout.lock().expect("playout").open = false;
}

fn drain<S: Copy>(playout: &Mutex<Playout>, data: &mut [S], as_sample: impl Fn(i16) -> S) {
    let mut playout = playout.lock().expect("playout");
    for slot in data.iter_mut() {
        *slot = as_sample(playout.samples.pop_front().unwrap_or(0));
    }
}

/// The mode asked for when the device offers it; its default otherwise. The
/// `audio.play` answer says which one opened, and the view writes at that.
fn output_config(
    device: &cpal::Device,
    want: &Speak,
) -> Result<cpal::SupportedStreamConfig, String> {
    use cpal::traits::DeviceTrait as _;
    let offered = device
        .supported_output_configs()
        .ok()
        .into_iter()
        .flatten()
        .find(|range| {
            readable(range.sample_format())
                && range.channels() == u16::from(want.channels)
                && (range.min_sample_rate().0..=range.max_sample_rate().0).contains(&want.rate)
        })
        .map(|range| range.with_sample_rate(cpal::SampleRate(want.rate)));
    match offered {
        Some(config) => Ok(config),
        None => device
            .default_output_config()
            .map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One item is 20 ms of whatever opened, across its channels.
    #[test]
    fn one_captured_item_is_twenty_milliseconds_of_the_opened_mode() {
        assert_eq!(
            chunk_samples(Opened {
                rate: 48_000,
                channels: 1
            }),
            960
        );
        assert_eq!(
            chunk_samples(Opened {
                rate: 48_000,
                channels: 2
            }),
            1_920
        );
        let mut buffer = Vec::new();
        let (out, mut taken) = tokio::sync::mpsc::channel(4);
        pump(&mut buffer, (0..959).map(|_| 7i16), 960, &out);
        assert!(taken.try_recv().is_err(), "a part item is not sent");
        pump(&mut buffer, std::iter::once(7i16), 960, &out);
        assert_eq!(
            taken.try_recv().unwrap().len(),
            960 * 2,
            "i16, so two bytes"
        );
    }

    /// `audio.play` takes a mode a device could plausibly open, and nothing
    /// else — a rate of zero would make the ceiling zero and every write a
    /// refusal nobody can act on.
    #[test]
    fn a_playout_mode_is_bounded_before_a_device_is_touched() {
        let playable = |body: &str| {
            serde_json::from_str::<Speak>(body).is_ok_and(|want| {
                (8_000..=192_000).contains(&want.rate) && (1..=8).contains(&want.channels)
            })
        };
        assert!(playable(r#"{"rate":48000,"channels":2}"#));
        assert!(!playable(r#"{"rate":0,"channels":1}"#));
        assert!(!playable(r#"{"rate":48000,"channels":0}"#));
        assert!(!playable(r#"{"rate":48000,"channels":2,"gain":3}"#));
    }

    /// A write past half a second of unplayed audio is REFUSED, and says so
    /// with a token the view can branch on; a write before `audio.play`, or
    /// after the device is gone, is refused too rather than swallowed.
    #[test]
    fn writing_past_the_playout_ceiling_is_refused_not_queued() {
        let mut media = Media::default();
        assert_eq!(written(&media, &[0; 4]).unwrap_err().reason, "not_open");
        let (stop, _stopped) = std::sync::mpsc::channel();
        let playout = Arc::new(Mutex::new(Playout {
            samples: VecDeque::new(),
            ceiling: ceiling(48_000, 1),
            open: true,
        }));
        media.out = Some(Out {
            playout: playout.clone(),
            _stop: stop,
        });
        assert_eq!(ceiling(48_000, 1), 24_000);
        assert_eq!(
            written(&media, &[0; 3]).unwrap_err().reason,
            "malformed_request"
        );
        written(&media, &vec![0; 24_000 * 2]).expect("half a second fits");
        assert_eq!(playout.lock().unwrap().samples.len(), 24_000);
        assert_eq!(written(&media, &[0; 2]).unwrap_err().reason, "backpressure");
        playout.lock().unwrap().open = false;
        assert_eq!(written(&media, &[0; 2]).unwrap_err().reason, "not_open");
    }

    /// Written samples reach the device in the order and the values they
    /// arrived in, little-endian and interleaved — the host transforms
    /// nothing.
    #[test]
    fn written_samples_reach_the_device_unchanged() {
        let mut media = Media::default();
        let (stop, _stopped) = std::sync::mpsc::channel();
        let playout = Arc::new(Mutex::new(Playout {
            samples: VecDeque::new(),
            ceiling: 8,
            open: true,
        }));
        media.out = Some(Out {
            playout: playout.clone(),
            _stop: stop,
        });
        let samples: [i16; 3] = [-32768, 0, 32767];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        written(&media, &bytes).expect("three samples fit");
        let mut data = [1i16; 4];
        drain(&playout, &mut data, |sample| sample);
        assert_eq!(data, [-32768, 0, 32767, 0], "and silence past the end");
    }
}
