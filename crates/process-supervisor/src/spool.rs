use std::collections::VecDeque;

use crate::{FramedEvent, ProcessEvent, Replay};

pub(crate) struct Spool {
    cap: usize,
    bytes: usize,
    next_offset: u64,
    dropped_frames: u64,
    dropped_bytes: u64,
    frames: VecDeque<FramedEvent>,
}

impl Spool {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            bytes: 0,
            next_offset: 0,
            dropped_frames: 0,
            dropped_bytes: 0,
            frames: VecDeque::new(),
        }
    }

    pub(crate) fn push(&mut self, event: ProcessEvent) -> FramedEvent {
        let frame = FramedEvent {
            offset: self.next_offset,
            event,
        };
        self.next_offset += 1;
        self.bytes += frame.output_len();
        self.frames.push_back(frame.clone());

        while self.bytes > self.cap {
            let Some(dropped) = self.frames.pop_front() else {
                break;
            };
            let dropped_bytes = dropped.output_len();
            self.bytes -= dropped_bytes;
            self.dropped_frames += 1;
            self.dropped_bytes += dropped_bytes as u64;
        }
        frame
    }

    pub(crate) fn replay(&self, requested_offset: u64) -> Replay {
        let oldest_offset = self
            .frames
            .front()
            .map_or(self.next_offset, |frame| frame.offset);
        Replay {
            requested_offset,
            oldest_offset,
            next_offset: self.next_offset,
            dropped_frames: self.dropped_frames,
            dropped_bytes: self.dropped_bytes,
            frames: self
                .frames
                .iter()
                .filter(|frame| frame.offset >= requested_offset)
                .cloned()
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputStream;

    fn output(bytes: &[u8]) -> ProcessEvent {
        ProcessEvent::Output {
            stream: OutputStream::Stdout,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn drops_oldest_frames_and_reports_the_gap() {
        let mut spool = Spool::new(5);
        spool.push(ProcessEvent::Started { pid: 7 });
        spool.push(output(b"abc"));
        spool.push(output(b"def"));

        let replay = spool.replay(0);
        assert_eq!(replay.oldest_offset, 2);
        assert_eq!(replay.next_offset, 3);
        assert_eq!(replay.dropped_frames, 2);
        assert_eq!(replay.dropped_bytes, 3);
        assert_eq!(replay.frames.len(), 1);
        assert!(replay.was_truncated());
    }
}
