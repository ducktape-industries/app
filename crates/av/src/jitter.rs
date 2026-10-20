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
/// A packet this far behind the next is a sender numbering again from
/// the start (one audio group), not a late one: play starts over.
pub const RESTART: u64 = 50;

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
    next: Option<u64>,
    depth: usize,
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
        if let Some(next) = self.next
            && seq < next
        {
            if seq + RESTART >= next {
                return;
            }
            self.held.clear();
            self.next = None;
            self.primed = false;
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
            self.primed = false;
            return Pop::Empty;
        }
        if self.held.len() > self.depth {
            let oldest = *self.held.keys().next().expect("held");
            self.next = Some(oldest);
            return Pop::Lost {
                next: self.held.get(&oldest).cloned(),
            };
        }
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
        assert_eq!(jitter.pop(), Pop::Empty);
        for seq in 6..9 {
            jitter.push(seq, packet(seq));
            assert_eq!(jitter.pop(), Pop::Empty);
        }
        jitter.push(9, packet(9));
        assert_eq!(jitter.pop(), Pop::Packet(packet(6)));
    }

    #[test]
    fn a_burst_after_a_stall_jumps_to_its_oldest() {
        let mut jitter = Jitter::default();
        for seq in 0..3 {
            jitter.push(seq, packet(seq));
        }
        for seq in 0..3 {
            assert_eq!(jitter.pop(), Pop::Packet(packet(seq)));
        }
        for seq in 10..20 {
            jitter.push(seq, packet(seq));
        }
        assert_eq!(
            jitter.pop(),
            Pop::Lost {
                next: Some(packet(10))
            }
        );
        assert_eq!(jitter.pop(), Pop::Packet(packet(10)));
        assert_eq!(jitter.depth, DEPTH, "no deeper for a jump");
    }

    #[test]
    fn a_sender_starting_over_is_played_again() {
        let mut jitter = Jitter::default();
        for seq in 200..203 {
            jitter.push(seq, packet(seq));
        }
        assert_eq!(jitter.pop(), Pop::Packet(packet(200)));
        jitter.push(199, packet(199));
        assert_eq!(jitter.pop(), Pop::Packet(packet(201)), "199 was late");
        for seq in 0..3 {
            jitter.push(seq, packet(seq));
        }
        assert_eq!(jitter.pop(), Pop::Packet(packet(0)));
    }
}
