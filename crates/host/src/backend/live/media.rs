//! A call's media on the wire, pure: how an encoded picture becomes the
//! objects of a stream track and comes back whole, how a voice's packets
//! are numbered on a datagram track, and the ladder a publisher's picture
//! steps down when the node cannot take it. No device, no codec, no
//! session: the pipeline (`render/call.rs`) feeds packets in and objects
//! out, and this is tested by itself.
//!
//! A `cam/<account>` track (stream mode): one group per GOP, opened by a
//! keyframe and closed (an empty object with `end`) when the next one
//! starts. Each object is one chunk of a frame: a [`HEADER`] of
//! `{ts_ms: u32 LE, flags: u8, part: u8}` and at most [`CHUNK`] bytes,
//! so a chunk never exceeds the node's object cap; a frame is the chunks
//! up to the one flagged [`LAST`]. A subscriber decodes nothing until the
//! first chunk flagged [`KEY`]; a frame cut short (a group ended
//! incomplete, a part missing) is dropped and the next keyframe waited for.
//!
//! A `mic/<account>` track (datagram mode): one 20 ms packet per object,
//! [`AUDIO_GROUP`] per group, `object` the packet's index in it.
use std::time::{Duration, Instant};

use super::{Data, Object};

pub(crate) const HEADER: usize = 6;
pub(crate) const CHUNK: usize = 60 * 1024;
const _: () = assert!(HEADER + CHUNK <= live_wire::MAX_STREAM_OBJECT);
const KEY: u8 = 1;
const LAST: u8 = 2;
pub(crate) const AUDIO_GROUP: u32 = 50;
pub(crate) const GOP: Duration = Duration::from_secs(2);
pub(crate) const FRESH: Duration = Duration::from_millis(500);
pub(crate) const CLEAN: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) ts_ms: u32,
    pub(crate) keyframe: bool,
    pub(crate) data: Vec<u8>,
}

pub(crate) fn chunk(frame: &Frame) -> Vec<Vec<u8>> {
    let parts = frame.data.chunks(CHUNK).count().max(1);
    frame
        .data
        .chunks(CHUNK)
        .chain(std::iter::once(&[][..]).take(usize::from(frame.data.is_empty())))
        .enumerate()
        .map(|(part, bytes)| {
            let mut body = Vec::with_capacity(HEADER + bytes.len());
            body.extend_from_slice(&frame.ts_ms.to_le_bytes());
            let last = if part + 1 == parts { LAST } else { 0 };
            let key = if frame.keyframe { KEY } else { 0 };
            body.push(key | last);
            body.push(part as u8);
            body.extend_from_slice(bytes);
            body
        })
        .collect()
}

#[derive(Default)]
pub(crate) struct Chunker {
    open: Option<(u64, u32, Instant)>,
    groups: u64,
}

impl Chunker {
    pub(crate) fn push(&mut self, frame: &Frame, now: Instant) -> Vec<Object> {
        let mut out = Vec::new();
        if frame.keyframe {
            if let Some((group, object, _)) = self.open.take() {
                out.push(Object {
                    group,
                    object,
                    body: Vec::new(),
                    end: true,
                });
            }
            self.open = Some((self.groups, 0, now));
            self.groups += 1;
        }
        let Some((group, object, _)) = &mut self.open else {
            return out;
        };
        for body in chunk(frame) {
            out.push(Object {
                group: *group,
                object: *object,
                body,
                end: false,
            });
            *object += 1;
        }
        out
    }

    pub(crate) fn age(&self, now: Instant) -> Option<Duration> {
        self.open.map(|(_, _, opened)| now.duration_since(opened))
    }

    pub(crate) fn cut(&mut self) {
        self.open = None;
    }
}

#[derive(Default)]
pub(crate) struct Reassembler {
    armed: bool,
    gathering: Option<Gathering>,
}

struct Gathering {
    group: u64,
    ts_ms: u32,
    keyframe: bool,
    next_part: u8,
    data: Vec<u8>,
}

impl Reassembler {
    pub(crate) fn push(&mut self, data: Data) -> Option<Frame> {
        match data {
            Data::GroupEnd { complete, .. } => {
                self.gathering = None;
                if !complete {
                    self.armed = false;
                }
                None
            }
            Data::Object { group, body, .. } => self.object(group, &body),
        }
    }

    fn object(&mut self, group: u64, body: &[u8]) -> Option<Frame> {
        let (header, bytes) = match body.split_at_checked(HEADER) {
            Some(split) => split,
            None => {
                self.gathering = None;
                return None;
            }
        };
        let ts_ms = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let (flags, part) = (header[4], header[5]);
        if part == 0 {
            if self.gathering.take().is_some() {
                self.armed = false;
            }
            let keyframe = flags & KEY != 0;
            if keyframe {
                self.armed = true;
            }
            if !self.armed {
                return None;
            }
            self.gathering = Some(Gathering {
                group,
                ts_ms,
                keyframe,
                next_part: 0,
                data: Vec::new(),
            });
        }
        let Some(gathering) = &mut self.gathering else {
            self.armed = false;
            return None;
        };
        if gathering.group != group || gathering.next_part != part {
            self.gathering = None;
            self.armed = false;
            return None;
        }
        gathering.data.extend_from_slice(bytes);
        gathering.next_part = part.wrapping_add(1);
        if flags & LAST == 0 {
            return None;
        }
        let done = self.gathering.take()?;
        Some(Frame {
            ts_ms: done.ts_ms,
            keyframe: done.keyframe,
            data: done.data,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.armed = false;
        self.gathering = None;
    }
}

#[derive(Default)]
pub(crate) struct AudioNumbering {
    next: u64,
}

impl AudioNumbering {
    pub(crate) fn next(&mut self) -> (u64, u32) {
        let at = self.next;
        self.next += 1;
        (
            at / u64::from(AUDIO_GROUP),
            (at % u64::from(AUDIO_GROUP)) as u32,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rung {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) fps: u32,
    pub(crate) bitrate: u32,
}

pub(crate) const RUNGS: [Rung; 3] = [
    Rung {
        width: 640,
        height: 360,
        fps: 15,
        bitrate: 600_000,
    },
    Rung {
        width: 480,
        height: 270,
        fps: 15,
        bitrate: 300_000,
    },
    Rung {
        width: 320,
        height: 180,
        fps: 12,
        bitrate: 150_000,
    },
];

pub(crate) struct Ladder {
    at: usize,
    clean_since: Instant,
}

impl Ladder {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            at: 0,
            clean_since: now,
        }
    }

    pub(crate) fn rung(&self) -> Rung {
        RUNGS[self.at]
    }

    pub(crate) fn lowered(&self) -> bool {
        self.at > 0
    }

    pub(crate) fn cut(&mut self, now: Instant) -> Option<Rung> {
        self.clean_since = now;
        if self.at + 1 >= RUNGS.len() {
            return None;
        }
        self.at += 1;
        Some(self.rung())
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Option<Rung> {
        if self.at == 0 || now.duration_since(self.clean_since) < CLEAN {
            return None;
        }
        self.at -= 1;
        self.clean_since = now;
        Some(self.rung())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(keyframe: bool, len: usize, ts_ms: u32) -> Frame {
        Frame {
            ts_ms,
            keyframe,
            data: (0..len).map(|at| (at % 251) as u8).collect(),
        }
    }

    fn objects_of(group: u64, bodies: Vec<Vec<u8>>) -> Vec<Data> {
        bodies
            .into_iter()
            .enumerate()
            .map(|(object, body)| Data::Object {
                group,
                object: object as u32,
                body,
            })
            .collect()
    }

    #[test]
    fn a_frame_round_trips_in_chunks() {
        let big = frame(true, 100 * 1024, 7);
        let chunks = chunk(&big);
        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .iter()
                .all(|c| c.len() <= live_wire::MAX_STREAM_OBJECT)
        );
        assert_eq!(chunks[0].len(), HEADER + CHUNK);
        let mut back = Reassembler::default();
        let mut got = objects_of(0, chunks)
            .into_iter()
            .filter_map(|data| back.push(data));
        assert_eq!(got.next(), Some(big.clone()));
        assert_eq!(got.next(), None);
        let small = frame(false, 300, 8);
        let chunks = chunk(&small);
        assert_eq!(chunks.len(), 1);
        for data in objects_of(0, chunks) {
            assert_eq!(back.push(data), Some(small.clone()));
        }
        let empty = frame(false, 0, 9);
        assert_eq!(chunk(&empty).len(), 1);
        assert_eq!(chunk(&empty)[0][4] & LAST, LAST);
    }

    #[test]
    fn a_frame_cut_short_waits_for_the_next_keyframe() {
        let mut back = Reassembler::default();
        for data in objects_of(0, chunk(&frame(false, 100, 1))) {
            assert_eq!(back.push(data), None, "a delta before a keyframe");
        }
        let key = frame(true, 70 * 1024, 2);
        let mut chunks = objects_of(1, chunk(&key));
        let last = chunks.pop().unwrap();
        for data in chunks {
            assert_eq!(back.push(data), None);
        }
        assert_eq!(
            back.push(Data::GroupEnd {
                group: 1,
                complete: false
            }),
            None
        );
        assert_eq!(back.push(last), None, "the late part completes nothing");
        for data in objects_of(2, chunk(&frame(false, 100, 3))) {
            assert_eq!(back.push(data), None, "a delta after the cut");
        }
        let next = frame(true, 100, 4);
        for data in objects_of(3, chunk(&next)) {
            assert_eq!(back.push(data), Some(next.clone()));
        }
        let delta = frame(false, 50, 5);
        for data in objects_of(3, chunk(&delta)) {
            assert_eq!(back.push(data), Some(delta.clone()));
        }
        let mut chunks = objects_of(3, chunk(&frame(false, 2 * CHUNK, 6)));
        chunks.remove(1);
        for data in chunks {
            assert_eq!(back.push(data), None);
        }
        for data in objects_of(3, chunk(&frame(false, 50, 7))) {
            assert_eq!(back.push(data), None);
        }
    }

    #[test]
    fn a_part_with_no_first_waits_for_the_next_keyframe() {
        let mut frames = Reassembler::default();
        let key = Frame {
            ts_ms: 0,
            keyframe: true,
            data: vec![1; 10],
        };
        let delta = Frame {
            ts_ms: 40,
            keyframe: false,
            data: vec![2; 10],
        };
        let object = |body: Vec<u8>| Data::Object {
            group: 0,
            object: 0,
            body,
        };
        assert!(frames.push(object(chunk(&key).remove(0))).is_some());
        let mut orphan = chunk(&delta).remove(0);
        orphan[5] = 1;
        assert!(frames.push(object(orphan)).is_none());
        assert!(
            frames.push(object(chunk(&delta).remove(0))).is_none(),
            "disarmed"
        );
        assert!(frames.push(object(chunk(&key).remove(0))).is_some());
    }

    #[test]
    fn a_keyframe_opens_a_group_and_closes_the_last() {
        let now = Instant::now();
        let mut chunker = Chunker::default();
        assert!(chunker.push(&frame(false, 10, 0), now).is_empty());
        let first = chunker.push(&frame(true, 10, 1), now);
        assert_eq!(first.len(), 1);
        assert_eq!(
            (first[0].group, first[0].object, first[0].end),
            (0, 0, false)
        );
        let delta = chunker.push(&frame(false, 10, 2), now);
        assert_eq!((delta[0].group, delta[0].object), (0, 1));
        let second = chunker.push(&frame(true, CHUNK + 1, 3), now + Duration::from_secs(1));
        assert_eq!(second.len(), 3);
        assert_eq!(
            (second[0].group, second[0].object, second[0].end),
            (0, 2, true)
        );
        assert!(second[0].body.is_empty());
        assert_eq!((second[1].group, second[1].object), (1, 0));
        assert_eq!((second[2].group, second[2].object), (1, 1));
        assert_eq!(
            chunker.age(now + Duration::from_secs(3)),
            Some(Duration::from_secs(2))
        );
        chunker.cut();
        assert!(chunker.push(&frame(false, 10, 4), now).is_empty());
        let third = chunker.push(&frame(true, 10, 5), now);
        assert_eq!(
            (third[0].group, third[0].object),
            (2, 0),
            "no end for a cut group"
        );
    }

    #[test]
    fn voice_packets_are_numbered_a_second_a_group() {
        let mut numbering = AudioNumbering::default();
        assert_eq!(numbering.next(), (0, 0));
        for _ in 1..AUDIO_GROUP - 1 {
            numbering.next();
        }
        assert_eq!(numbering.next(), (0, AUDIO_GROUP - 1));
        assert_eq!(numbering.next(), (1, 0));
    }

    #[test]
    fn the_ladder_steps_down_on_a_cut_and_up_when_clean() {
        let start = Instant::now();
        let mut ladder = Ladder::new(start);
        assert_eq!(ladder.rung(), RUNGS[0]);
        assert!(!ladder.lowered());
        assert_eq!(ladder.cut(start), Some(RUNGS[1]));
        assert!(ladder.lowered());
        assert_eq!(ladder.cut(start), Some(RUNGS[2]));
        assert_eq!(ladder.cut(start), None);
        assert_eq!(ladder.tick(start + CLEAN - Duration::from_millis(1)), None);
        assert_eq!(ladder.tick(start + CLEAN), Some(RUNGS[1]));
        assert_eq!(ladder.tick(start + CLEAN), None, "the clock restarts");
        assert_eq!(ladder.tick(start + 2 * CLEAN), Some(RUNGS[0]));
        assert_eq!(ladder.tick(start + 3 * CLEAN), None);
    }
}
