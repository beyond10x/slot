//! One GNU-make jobserver backed by a named FIFO.
//!
//! cargo, rustc and make read and write the FIFO directly. The daemon takes
//! tokens for jobs in arrival order through one reader thread.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, poll};
use nix::sys::stat::Mode;
use tokio::sync::oneshot;

/// How long the reader waits for a token before it looks at its flags again.
const READ_POLL_MS: u16 = 100;

/// The token pool.
pub struct Pool {
    path: PathBuf,
    size: usize,
    file: File,
    inner: Mutex<Inner>,
    wake: Condvar,
    parked: Condvar,
}

#[derive(Default)]
struct Inner {
    queue: VecDeque<Arc<Mutex<WaiterState>>>,
    /// Tokens the daemon keeps out of circulation: size minus the limit.
    reserve: usize,
    /// Tokens the daemon holds back now; grows to `reserve` as tokens return.
    held_back: usize,
    /// Set by [`Pool::pause`]: the reader takes no token until it is cleared.
    pause: bool,
    /// The reader is stopped between two tokens, holding none in flight.
    reader_parked: bool,
}

/// What a handover passes on about the pool besides the FIFO itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolState {
    /// Pool size.
    pub size: usize,
    /// Tokens kept out of circulation.
    pub reserve: usize,
    /// Tokens the daemon held back in memory, out of the FIFO.
    pub held_back: usize,
}

/// A handle on a queued request's tokens, kept beside the job so a handover
/// can take back what the request gathered.
#[derive(Clone)]
pub struct WaiterHandle(Arc<Mutex<WaiterState>>);

impl WaiterHandle {
    /// Takes the tokens the request holds and withdraws it: from now on it
    /// holds none and is granted nothing.
    pub fn take(&self) -> usize {
        let mut st = self.0.lock().expect("waiter lock");
        st.cancelled = true;
        std::mem::take(&mut st.got)
    }
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

    /// Tokens this waiter holds now. After [`Waiter::ready`] that is the
    /// request, or less when a lower limit shrank it while it was queued.
    pub fn held(&self) -> usize {
        self.state.lock().expect("waiter lock").got
    }

    /// Takes the tokens this waiter holds: what to release when it does not
    /// go on to run. A second call, or one after [`WaiterHandle::take`],
    /// returns 0, so no token is released twice.
    pub fn take_held(&self) -> usize {
        std::mem::take(&mut self.state.lock().expect("waiter lock").got)
    }

    /// A handle on this waiter's tokens.
    pub fn handle(&self) -> WaiterHandle {
        WaiterHandle(Arc::clone(&self.state))
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
        let pool = Self::open(path, size, Inner::default())?;
        pool.write(size);
        Ok(pool)
    }

    /// Takes over the FIFO at `path` from a daemon that re-executed: writes
    /// no token, since every token is in the FIFO, held by a running job, or
    /// held back as `state` says. `inherited` is the old process's open
    /// descriptor of the FIFO, which kept its tokens across exec; it is
    /// closed once the FIFO is open again.
    pub fn adopt(path: &Path, state: PoolState, inherited: Option<OwnedFd>) -> Result<Arc<Self>> {
        if state.size < 1 || state.reserve >= state.size || state.held_back > state.reserve {
            bail!("jobserver: inconsistent handover state {state:?}");
        }
        let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        if !std::os::unix::fs::FileTypeExt::is_fifo(&meta.file_type()) {
            bail!("jobserver: {} is not a FIFO", path.display());
        }
        let pool = Self::open(
            path,
            state.size,
            Inner {
                reserve: state.reserve,
                held_back: state.held_back,
                ..Inner::default()
            },
        );
        drop(inherited);
        pool
    }

    fn open(path: &Path, size: usize, inner: Inner) -> Result<Arc<Self>> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        // The reader polls, so it can stop between two tokens; other
        // processes open the FIFO by path and keep their own flags.
        fcntl(file.as_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
            .with_context(|| format!("set O_NONBLOCK on {}", path.display()))?;
        let pool = Arc::new(Self {
            path: path.to_path_buf(),
            size,
            file,
            inner: Mutex::new(inner),
            wake: Condvar::new(),
            parked: Condvar::new(),
        });
        let reader = Arc::clone(&pool);
        std::thread::Builder::new()
            .name("jobserver".into())
            .spawn(move || reader.reader())?;
        Ok(pool)
    }

    /// Stops the reader between two tokens, so no token is in flight inside
    /// the daemon. False when it did not stop within `timeout`.
    pub fn pause(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner.lock().expect("jobserver lock");
        inner.pause = true;
        self.wake.notify_all();
        while !inner.reader_parked {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            inner = self
                .parked
                .wait_timeout(inner, left)
                .expect("jobserver wait")
                .0;
        }
        true
    }

    /// Lets the reader take tokens again after [`Pool::pause`].
    pub fn resume(&self) {
        let mut inner = self.inner.lock().expect("jobserver lock");
        inner.pause = false;
        drop(inner);
        self.wake.notify_all();
    }

    /// What a handover passes on: size, reserve and the tokens held back.
    pub fn state(&self) -> PoolState {
        let inner = self.inner.lock().expect("jobserver lock");
        PoolState {
            size: self.size,
            reserve: inner.reserve,
            held_back: inner.held_back,
        }
    }

    /// The daemon's open descriptor of the FIFO.
    pub fn file(&self) -> &File {
        &self.file
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

    /// Tokens in circulation once every withheld token is back: the pool
    /// size minus the reserve.
    pub fn limit(&self) -> usize {
        self.size - self.inner.lock().expect("jobserver lock").reserve
    }

    /// Tokens the daemon holds back now.
    pub fn held_back(&self) -> usize {
        self.inner.lock().expect("jobserver lock").held_back
    }

    /// Sets how many tokens circulate, 1 to the pool size. Lowering it takes
    /// tokens out as running jobs return them (a token a process holds
    /// cannot be revoked); raising it writes held-back tokens back at once.
    /// Queued requests larger than the new limit shrink to it.
    pub fn set_limit(&self, limit: usize) -> usize {
        let limit = limit.clamp(1, self.size);
        let mut inner = self.inner.lock().expect("jobserver lock");
        inner.reserve = self.size - limit;
        if inner.held_back > inner.reserve {
            let back = inner.held_back - inner.reserve;
            inner.held_back = inner.reserve;
            self.write(back);
        }
        let mut done = false;
        if let Some(h) = inner.queue.front() {
            let mut st = h.lock().expect("waiter lock");
            if !st.cancelled && !st.granted && st.n > limit {
                st.n = limit;
                if st.got >= st.n {
                    let surplus = st.got - st.n;
                    st.got = st.n;
                    st.granted = true;
                    if let Some(tx) = st.tx.take() {
                        let _ = tx.send(());
                    }
                    self.write(surplus);
                    done = true;
                }
            }
        }
        if done {
            inner.queue.pop_front();
        }
        for w in inner.queue.iter().skip(usize::from(!done)) {
            let mut st = w.lock().expect("waiter lock");
            st.n = st.n.min(limit);
        }
        drop(inner);
        self.wake.notify_one();
        limit
    }

    /// Queues a request for `n` tokens (capped at the limit).
    pub fn acquire(&self, n: usize) -> Waiter {
        let n = n.clamp(1, self.limit());
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
        let diff = (self.size - inner.held_back) as i64 - self.free() as i64;
        if diff > 0 {
            self.write(diff as usize);
            return diff;
        }
        if diff < 0 {
            // Non-blocking: take what is there, at most the surplus.
            let mut buf = vec![0u8; (-diff) as usize];
            let mut taken = 0;
            while taken < buf.len() {
                match (&self.file).read(&mut buf[taken..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => taken += n,
                }
            }
            return -(taken as i64);
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
                loop {
                    if inner.pause {
                        if !inner.reader_parked {
                            inner.reader_parked = true;
                            self.parked.notify_all();
                        }
                    } else {
                        inner.reader_parked = false;
                        if Self::head(&mut inner).is_some() || inner.held_back < inner.reserve {
                            break;
                        }
                    }
                    inner = self.wake.wait(inner).expect("jobserver wait");
                }
            }
            let mut fds = [PollFd::new(self.file.as_fd(), PollFlags::POLLIN)];
            match poll(&mut fds, READ_POLL_MS) {
                Ok(0) | Err(nix::errno::Errno::EINTR) => continue,
                Ok(_) => {}
                Err(_) => return,
            }
            match (&self.file).read(&mut buf) {
                Ok(1) => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    continue;
                }
                Ok(_) | Err(_) => return,
            }
            let mut inner = self.inner.lock().expect("jobserver lock");
            if inner.held_back < inner.reserve {
                inner.held_back += 1;
                continue;
            }
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
    async fn limit_withholds_and_returns_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 4).unwrap();
        let mut a = p.acquire(3);
        assert!(ready_within(&mut a, 1000).await);
        assert_eq!(p.set_limit(2), 2);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(p.free(), 0, "the one free token is withheld");
        assert_eq!(p.held_back(), 1);
        p.release(3);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(p.held_back(), 2, "shrinks as tokens return");
        assert_eq!(p.free(), 2);
        let mut b = p.acquire(4);
        assert_eq!(b.tokens(), 2, "requests are capped at the limit");
        assert!(ready_within(&mut b, 1000).await);
        p.release(2);
        assert_eq!(p.set_limit(4), 4);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(p.held_back(), 0);
        assert_eq!(p.free(), 4);
        assert_eq!(p.reconcile(), 0);
    }

    #[tokio::test]
    async fn lowering_the_limit_shrinks_a_queued_request() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 4).unwrap();
        let mut a = p.acquire(2);
        assert!(ready_within(&mut a, 1000).await);
        let mut b = p.acquire(4);
        assert!(!ready_within(&mut b, 200).await);
        p.set_limit(2);
        assert!(ready_within(&mut b, 1000).await, "2 of 2 is enough now");
        assert_eq!((b.tokens(), b.held()), (4, 2), "held is what to release");
        p.release(2);
        p.release(2);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(p.free() + p.held_back(), 4);
    }

    #[tokio::test]
    async fn a_paused_reader_grants_nothing_until_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        assert!(p.pause(Duration::from_secs(2)));
        let mut a = p.acquire(1);
        assert!(!ready_within(&mut a, 300).await, "granted while paused");
        assert_eq!(p.free(), 2, "no token left the FIFO");
        p.resume();
        assert!(ready_within(&mut a, 1000).await);
        p.release(1);
    }

    #[tokio::test]
    async fn adopt_takes_over_the_fifo_without_writing_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo");
        let old = Pool::new(&path, 4).unwrap();
        old.set_limit(3);
        let mut a = old.acquire(2);
        assert!(ready_within(&mut a, 1000).await);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(old.pause(Duration::from_secs(2)));
        let st = old.state();
        assert_eq!(
            st,
            PoolState {
                size: 4,
                reserve: 1,
                held_back: 1
            }
        );
        let new = Pool::adopt(&path, st, None).unwrap();
        assert!(old.pause(Duration::from_secs(2)));
        assert_eq!(new.free(), 1, "4 - 2 held by the job - 1 held back");
        new.release(a.take_held());
        assert_eq!(a.take_held(), 0, "a second take releases nothing");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(new.free() + new.held_back(), 4);
        assert_eq!(new.reconcile(), 0);
        assert!(
            Pool::adopt(
                &path,
                PoolState {
                    size: 4,
                    reserve: 4,
                    held_back: 0
                },
                None
            )
            .is_err()
        );
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
