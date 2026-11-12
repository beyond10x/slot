//! One GNU-make jobserver backed by a named FIFO.
//!
//! cargo, rustc and make read and write the FIFO directly. The daemon takes
//! tokens for jobs in arrival order through one reader thread.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context as _, Result, bail};
use nix::sys::stat::Mode;
use tokio::sync::oneshot;

/// The token pool.
pub struct Pool {
    path: PathBuf,
    size: usize,
    file: File,
    inner: Mutex<Inner>,
    wake: Condvar,
}

#[derive(Default)]
struct Inner {
    queue: VecDeque<Arc<Mutex<WaiterState>>>,
}

struct WaiterState {
    n: usize,
    got: usize,
    cancelled: bool,
    granted: bool,
    tx: Option<oneshot::Sender<()>>,
}

/// A queued request for tokens.
pub struct Waiter {
    state: Arc<Mutex<WaiterState>>,
    rx: Option<oneshot::Receiver<()>>,
    n: usize,
}

impl Waiter {
    /// Tokens requested.
    pub fn tokens(&self) -> usize {
        self.n
    }

    /// Resolves when every requested token is held.
    pub async fn ready(&mut self) {
        if let Some(rx) = self.rx.take() {
            let _ = rx.await;
        }
    }
}

impl Pool {
    /// Creates the FIFO at `path` holding `size` tokens and starts the reader.
    pub fn new(path: &Path, size: usize) -> Result<Arc<Self>> {
        if size < 1 {
            bail!("jobserver: size must be at least 1");
        }
        let _ = std::fs::remove_file(path);
        nix::unistd::mkfifo(path, Mode::from_bits_truncate(0o600))
            .with_context(|| format!("mkfifo {}", path.display()))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        let pool = Arc::new(Self {
            path: path.to_path_buf(),
            size,
            file,
            inner: Mutex::new(Inner::default()),
            wake: Condvar::new(),
        });
        pool.write(size);
        let reader = Arc::clone(&pool);
        std::thread::Builder::new()
            .name("jobserver".into())
            .spawn(move || reader.reader())?;
        Ok(pool)
    }

    /// FIFO path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Pool size.
    pub fn size(&self) -> usize {
        self.size
    }

    /// The value for `--jobserver-auth`.
    pub fn auth(&self) -> String {
        format!("fifo:{}", self.path.display())
    }

    /// Queues a request for `n` tokens (capped at the pool size).
    pub fn acquire(&self, n: usize) -> Waiter {
        let n = n.clamp(1, self.size);
        let (tx, rx) = oneshot::channel();
        let state = Arc::new(Mutex::new(WaiterState {
            n,
            got: 0,
            cancelled: false,
            granted: false,
            tx: Some(tx),
        }));
        let mut inner = self.inner.lock().expect("jobserver lock");
        inner.queue.push_back(Arc::clone(&state));
        drop(inner);
        self.wake.notify_one();
        Waiter {
            state,
            rx: Some(rx),
            n,
        }
    }

    /// Withdraws a waiter. Tokens it already took go back to the pool.
    /// Returns true when the waiter had been granted all its tokens.
    pub fn cancel(&self, w: &Waiter) -> bool {
        let _inner = self.inner.lock().expect("jobserver lock");
        let mut st = w.state.lock().expect("waiter lock");
        if st.granted {
            return true;
        }
        st.cancelled = true;
        let got = std::mem::take(&mut st.got);
        drop(st);
        self.write(got);
        false
    }

    /// Returns `n` tokens.
    pub fn release(&self, n: usize) {
        let _inner = self.inner.lock().expect("jobserver lock");
        self.write(n);
    }

    fn write(&self, n: usize) {
        if n > 0 {
            let _ = (&self.file).write_all(&vec![b'+'; n]);
        }
    }

    /// Tokens sitting in the FIFO now.
    pub fn free(&self) -> usize {
        rustix::io::ioctl_fionread(&self.file).map_or(0, |n| n as usize)
    }

    /// Jobs queued for tokens.
    pub fn waiting(&self) -> usize {
        let inner = self.inner.lock().expect("jobserver lock");
        inner
            .queue
            .iter()
            .filter(|w| !w.lock().expect("waiter lock").cancelled)
            .count()
    }

    /// Sets the FIFO back to `size` tokens. Call only when no job runs and
    /// none waits: then every token must be in the FIFO. A shortfall means a
    /// killed process took tokens with it; a surplus means a double release.
    /// Returns tokens added (positive) or removed (negative).
    pub fn reconcile(&self) -> i64 {
        let inner = self.inner.lock().expect("jobserver lock");
        if !inner.queue.is_empty() {
            return 0;
        }
        let diff = self.size as i64 - self.free() as i64;
        if diff > 0 {
            self.write(diff as usize);
            return diff;
        }
        if diff < 0 {
            let mut buf = vec![0u8; (-diff) as usize];
            if (&self.file).read_exact(&mut buf).is_err() {
                return 0;
            }
        }
        diff
    }

    fn head(inner: &mut Inner) -> Option<Arc<Mutex<WaiterState>>> {
        while let Some(h) = inner.queue.front() {
            if h.lock().expect("waiter lock").cancelled {
                inner.queue.pop_front();
            } else {
                return Some(Arc::clone(h));
            }
        }
        None
    }

    fn reader(&self) {
        let mut buf = [0u8; 1];
        loop {
            {
                let mut inner = self.inner.lock().expect("jobserver lock");
                while Self::head(&mut inner).is_none() {
                    inner = self.wake.wait(inner).expect("jobserver wait");
                }
            }
            if (&self.file).read_exact(&mut buf).is_err() {
                return;
            }
            let mut inner = self.inner.lock().expect("jobserver lock");
            match Self::head(&mut inner) {
                None => self.write(1),
                Some(h) => {
                    let mut st = h.lock().expect("waiter lock");
                    st.got += 1;
                    if st.got == st.n {
                        st.granted = true;
                        if let Some(tx) = st.tx.take() {
                            let _ = tx.send(());
                        }
                        drop(st);
                        inner.queue.pop_front();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn ready_within(w: &mut Waiter, ms: u64) -> bool {
        tokio::time::timeout(Duration::from_millis(ms), w.ready())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn order_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 3).unwrap();
        let mut a = p.acquire(2);
        assert!(ready_within(&mut a, 1000).await);
        let mut b = p.acquire(2);
        assert!(
            !ready_within(&mut b, 200).await,
            "got 2 tokens while 1 is free"
        );
        p.release(2);
        assert!(ready_within(&mut b, 1000).await);
        p.release(2);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(p.free(), 3);
    }

    #[tokio::test]
    async fn cancel_returns_partial_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let b = p.acquire(2);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!p.cancel(&b));
        p.release(1);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(p.free(), 2);
    }

    #[tokio::test]
    async fn reconcile_restores_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 4).unwrap();
        let mut a = p.acquire(3);
        assert!(ready_within(&mut a, 1000).await);
        assert_eq!(p.reconcile(), 3);
        assert_eq!(p.free(), 4);
        p.release(3);
        assert_eq!(p.reconcile(), -3);
        assert_eq!(p.free(), 4);
    }

    #[tokio::test]
    async fn request_capped_at_pool_size() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(4);
        assert_eq!(a.tokens(), 2);
        assert!(ready_within(&mut a, 1000).await);
    }
}
