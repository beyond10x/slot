//! The tail of one job's output, with live followers.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use tokio::sync::mpsc;

use crate::types::Chunk;

/// Bounded output buffer.
pub struct Ring {
    inner: Mutex<Inner>,
    /// When the job's output first showed a test binary start
    /// ([`crate::phases::starts_tests`]), Unix ms; 0 until then.
    tests_at: AtomicI64,
}

struct Inner {
    limit: usize,
    size: usize,
    chunks: VecDeque<Chunk>,
    /// Chunks written since the start, dropped ones included.
    written: u64,
    closed: bool,
    subs: Vec<mpsc::UnboundedSender<Chunk>>,
}

impl Ring {
    /// A ring keeping about `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                limit,
                size: 0,
                chunks: VecDeque::new(),
                written: 0,
                closed: false,
                subs: Vec::new(),
            }),
            tests_at: AtomicI64::new(0),
        }
    }

    /// Records that the job's first test binary started at `ms`, unless an
    /// earlier one was recorded.
    pub fn mark_tests(&self, ms: i64) {
        let _ = self
            .tests_at
            .compare_exchange(0, ms, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Forgets the recorded start of the tests, for a job that runs again.
    pub fn unmark_tests(&self) {
        self.tests_at.store(0, Ordering::SeqCst);
    }

    /// When the job's first test binary started, Unix ms.
    pub fn tests_at(&self) -> Option<i64> {
        match self.tests_at.load(Ordering::SeqCst) {
            0 => None,
            t => Some(t),
        }
    }

    pub(super) fn from_tail(limit: usize, chunks: Vec<Chunk>) -> Self {
        let ring = Self::new(limit);
        for chunk in chunks {
            ring.write(&chunk.t, chunk.b);
        }
        ring
    }

    pub(super) fn snapshot(&self) -> Vec<Chunk> {
        self.inner
            .lock()
            .expect("ring lock")
            .chunks
            .iter()
            .cloned()
            .collect()
    }

    /// Appends a chunk and forwards it to followers.
    pub fn write(&self, t: &str, b: String) {
        let mut g = self.inner.lock().expect("ring lock");
        let c = Chunk {
            t: t.to_string(),
            b,
        };
        g.size += c.b.len();
        g.written += 1;
        g.chunks.push_back(c.clone());
        while g.size > g.limit && g.chunks.len() > 1 {
            if let Some(old) = g.chunks.pop_front() {
                g.size -= old.b.len();
            }
        }
        g.subs.retain(|s| s.send(c.clone()).is_ok());
    }

    /// The buffered tail and a receiver of later chunks; the receiver ends
    /// when the job ends.
    pub fn follow(&self) -> (Vec<Chunk>, mpsc::UnboundedReceiver<Chunk>) {
        let mut g = self.inner.lock().expect("ring lock");
        let tail = g.chunks.iter().cloned().collect();
        let (tx, rx) = mpsc::unbounded_channel();
        if !g.closed {
            g.subs.push(tx);
        }
        (tail, rx)
    }

    /// The kept chunks from chunk number `from` on, every chunk written
    /// numbered from 0.
    pub fn since(&self, from: u64) -> crate::types::OutputTail {
        let g = self.inner.lock().expect("ring lock");
        let first = g.written - g.chunks.len() as u64;
        let start = from.clamp(first, g.written);
        crate::types::OutputTail {
            from: start,
            next: g.written,
            chunks: g
                .chunks
                .iter()
                .skip((start - first) as usize)
                .cloned()
                .collect(),
            ended: g.closed,
            kept: true,
        }
    }

    /// Marks the job ended and ends every follower.
    pub fn close(&self) {
        let mut g = self.inner.lock().expect("ring lock");
        g.closed = true;
        g.subs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_tail_and_ends_followers() {
        let r = Ring::new(10);
        r.write("out", "12345".into());
        let (tail, mut rx) = r.follow();
        assert_eq!(tail.len(), 1);
        r.write("err", "67890abc".into());
        assert_eq!(rx.try_recv().unwrap().b, "67890abc");
        r.close();
        assert!(rx.try_recv().is_err());
        let (tail, _) = r.follow();
        assert_eq!(tail.len(), 1, "oldest chunk dropped over the limit");
    }

    #[test]
    fn the_tail_from_a_chunk_on_numbers_every_chunk_written() {
        let r = Ring::new(10);
        r.write("out", "12345".into());
        r.write("out", "678".into());
        let t = r.since(0);
        assert_eq!((t.from, t.next, t.chunks.len(), t.ended), (0, 2, 2, false));
        let t = r.since(1);
        assert_eq!((t.from, t.next), (1, 2));
        assert_eq!(t.chunks[0].b, "678");
        r.write("err", "abcdefgh".into());
        r.close();
        let t = r.since(0);
        assert_eq!(
            (t.from, t.next, t.chunks.len(), t.ended),
            (2, 3, 1, true),
            "dropped chunks are skipped"
        );
        assert_eq!(r.since(3).chunks, []);
    }
}
