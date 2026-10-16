//! A remote voice's packets put back in order and paced to the speaker:
//! [`Jitter`] holds a few packets ([`DEPTH`] to start) so a packet that
//! comes late or out of order still plays in its place, grows by one on
//! each loss (to [`MAX_DEPTH`]) and says which packet did not come so the
//! decoder conceals it, with the next packet's FEC when that one is here.
//! Pure: the mixer pulls one packet per 20 ms frame.
use std::collections::BTreeMap;

/// Packets held before the first plays: 60 ms.
pub const DEPTH: usize = 3;
/// The most held, after losses: 200 ms.
pub const MAX_DEPTH: usize = 10;

/// What the speaker gets for one frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Pop {
    Packet(Vec<u8>),
    /// The packet did not come; `next` is the one after it, if here (its
    /// FEC rebuilds the lost one).
    Lost {
        next: Option<Vec<u8>>,
    },
    /// Nothing to play yet: silence while the buffer fills.
    Empty,
}

pub struct Jitter {
    held: BTreeMap<u64, Vec<u8>>,
    /// The sequence to play next; `None` before the first packet.
    next: Option<u64>,
    depth: usize,
    /// Filled to `depth` once: plays until it runs dry.
    primed: bool,
}

impl Default for Jitter {
    fn default() -> Self {
        Self {
            held: BTreeMap::new(),
            next: None,
            depth: DEPTH,
            primed: false,
        }
    }
}

impl Jitter {
    /// `packet` numbered `seq` by its sender; one that is late (its place
    /// already played) is dropped.
    pub fn push(&mut self, seq: u64, packet: Vec<u8>) {
        if self.next.is_some_and(|next| seq < next) {
            return;
        }
        self.held.insert(seq, packet);
    }

    /// The next frame's packet.
    pub fn pop(&mut self) -> Pop {
        if !self.primed {
            if self.held.len() < self.depth {
                return Pop::Empty;
            }
            self.primed = true;
            // the oldest held is where play starts (after a dry spell, the
            // sender has moved on)
            self.next = self.held.keys().next().copied();
        }
        let Some(next) = self.next else {
            return Pop::Empty;
        };
        if let Some(packet) = self.held.remove(&next) {
            self.next = Some(next + 1);
            return Pop::Packet(packet);
        }
        if self.held.is_empty() {
            // ran dry: silence until it fills again
            self.primed = false;
            return Pop::Empty;
        }
        // ponytail: the depth only grows; shrinking it back once the path
        // is clean again is the upgrade if the lag is ever heard
        self.depth = (self.depth + 1).min(MAX_DEPTH);
        self.next = Some(next + 1);
        Pop::Lost {
            next: self.held.get(&(next + 1)).cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seq: u64) -> Vec<u8> {
        vec![seq as u8]
    }

    /// Nothing plays until three are held; packets out of order play in
    /// order; one that never comes is told lost with the next one's FEC,
    /// and the buffer holds one more from then on; a late one is dropped.
    #[test]
    fn reorders_and_conceals_one_loss() {
        let mut jitter = Jitter::default();
        jitter.push(1, packet(1));
        assert_eq!(jitter.pop(), Pop::Empty);
        jitter.push(0, packet(0));
        assert_eq!(jitter.pop(), Pop::Empty);
        jitter.push(2, packet(2));
        assert_eq!(jitter.pop(), Pop::Packet(packet(0)));
        assert_eq!(jitter.pop(), Pop::Packet(packet(1)));
        // 3 is lost; 4 and 5 come
        jitter.push(4, packet(4));
        jitter.push(5, packet(5));
        assert_eq!(jitter.pop(), Pop::Packet(packet(2)));
        assert_eq!(
            jitter.pop(),
            Pop::Lost {
                next: Some(packet(4))
            }
        );
        assert_eq!(jitter.depth, DEPTH + 1);
        jitter.push(3, packet(3));
        assert_eq!(
            jitter.pop(),
            Pop::Packet(packet(4)),
            "the late one is dropped"
        );
        assert_eq!(jitter.pop(), Pop::Packet(packet(5)));
        // dry: silence, then it fills to the deeper depth before it plays
        assert_eq!(jitter.pop(), Pop::Empty);
        for seq in 6..9 {
            jitter.push(seq, packet(seq));
            assert_eq!(jitter.pop(), Pop::Empty);
        }
        jitter.push(9, packet(9));
        assert_eq!(jitter.pop(), Pop::Packet(packet(6)));
    }
}
