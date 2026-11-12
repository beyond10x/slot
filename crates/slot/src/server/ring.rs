//! The tail of one job's output, with live followers.

use std::collections::VecDeque;
use std::sync::Mutex;

use tokio::sync::mpsc;

use crate::types::Chunk;

/// Bounded output buffer.
pub struct Ring {
    inner: Mutex<Inner>,
}

struct Inner {
    limit: usize,
    size: usize,
    chunks: VecDeque<Chunk>,
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
                closed: false,
                subs: Vec::new(),
            }),
        }
    }

    /// Appends a chunk and forwards it to followers.
    pub fn write(&self, t: &str, b: String) {
        let mut g = self.inner.lock().expect("ring lock");
        let c = Chunk {
            t: t.to_string(),
            b,
        };
        g.size += c.b.len();
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
}
