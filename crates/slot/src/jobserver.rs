//! One GNU-make jobserver backed by a named FIFO.
//!
//! cargo, rustc and make read and write the FIFO directly. The daemon takes
//! tokens for jobs through one reader thread, for one request at a time: the
//! head of the queue, which is ordered by [`Priority`], then by arrival. The
//! head gathers tokens until it has all it asked for; no request behind it
//! takes one meanwhile, so a request for many tokens is not overtaken by a
//! stream of small ones of its own level or below. Only the head ever holds
//! tokens: when a request of a higher level arrives or is raised ahead of
//! it, the old head's tokens move to the new one. An `urgent` head with no
//! token free borrows the tokens the headroom holds back.

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

use crate::types::Priority;

/// How long the reader waits for a token before it looks at its flags again.
const READ_POLL_MS: u16 = 100;
/// Processes other than `own` that have the FIFO at `path` open, from
/// `/proc/<pid>/fd`. Processes of other users are not readable and not
/// counted; the FIFO is mode 0600, so they cannot open it either.
pub fn holders(path: &Path, own: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return out;
    };
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if pid == own {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t == path))
        {
            out.push(pid);
        }
    }
    out
}

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
    /// Arrival number of the next request: first in, first out inside a level.
    next_seq: u64,
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
    level: Priority,
    seq: u64,
}

impl WaiterState {
    /// Where the request stands in the queue: level first, then arrival.
    fn rank(&self) -> (Priority, u64) {
        (self.level, self.seq)
    }

    /// Marks the request granted and wakes its job.
    fn grant(&mut self) {
        self.granted = true;
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
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

    /// Resolves when every requested token is held. Cancel-safe: dropped
    /// before it resolves, a later call still waits.
    pub async fn ready(&mut self) {
        if let Some(rx) = self.rx.as_mut() {
            let _ = rx.await;
            self.rx = None;
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
        if let Some(h) = Self::head(&mut inner) {
            let mut st = h.lock().expect("waiter lock");
            if !st.cancelled && !st.granted && st.n > limit {
                st.n = limit;
                if st.got >= st.n {
                    let surplus = st.got - st.n;
                    st.got = st.n;
                    st.grant();
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

    /// Queues a request for `n` tokens (capped at the limit) at level `normal`.
    pub fn acquire(&self, n: usize) -> Waiter {
        self.acquire_at(n, Priority::Normal)
    }

    /// Queues a request for `n` tokens (capped at the limit) at queue level
    /// `level`: behind every request of its level or a higher one, ahead of
    /// every request of a lower one.
    pub fn acquire_at(&self, n: usize, level: Priority) -> Waiter {
        let n = n.clamp(1, self.limit());
        let (tx, rx) = oneshot::channel();
        let mut inner = self.inner.lock().expect("jobserver lock");
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let state = Arc::new(Mutex::new(WaiterState {
            n,
            got: 0,
            cancelled: false,
            granted: false,
            tx: Some(tx),
            level,
            seq,
        }));
        let at = inner
            .queue
            .iter()
            .position(|w| w.lock().expect("waiter lock").level > level)
            .unwrap_or(inner.queue.len());
        inner.queue.insert(at, Arc::clone(&state));
        self.regather(&mut inner);
        self.lend(&mut inner);
        drop(inner);
        self.wake.notify_one();
        Waiter {
            state,
            rx: Some(rx),
            n,
        }
    }

    /// Changes the level of a queued request; it keeps its arrival order
    /// inside the new level. A request already granted is left alone.
    pub fn set_level(&self, w: &WaiterHandle, level: Priority) {
        let mut inner = self.inner.lock().expect("jobserver lock");
        {
            let mut st = w.0.lock().expect("waiter lock");
            if st.level == level || st.granted {
                return;
            }
            st.level = level;
        }
        inner
            .queue
            .make_contiguous()
            .sort_by_key(|w| w.lock().expect("waiter lock").rank());
        self.regather(&mut inner);
        self.lend(&mut inner);
        drop(inner);
        self.wake.notify_one();
    }

    /// Queued requests served before a new request at `level`: those of
    /// that level and the higher ones.
    pub fn waiting_before(&self, level: Priority) -> usize {
        let inner = self.inner.lock().expect("jobserver lock");
        inner
            .queue
            .iter()
            .map(|w| w.lock().expect("waiter lock"))
            .filter(|w| !w.cancelled && w.level <= level)
            .count()
    }

    /// Keeps every gathered token with the head of the queue: tokens that
    /// requests behind it took while they were the head move to it, the
    /// head is granted once it has all, the rest goes on to the next head,
    /// and what nobody needs goes back to the FIFO. Without it a request
    /// pushed back by a higher one would sit on tokens the new head waits
    /// for, and two such could each hold part of the pool forever.
    fn regather(&self, inner: &mut Inner) {
        if Self::head(inner).is_none() {
            return;
        }
        let mut spare = 0;
        for w in inner.queue.iter().skip(1) {
            let mut st = w.lock().expect("waiter lock");
            if !st.cancelled && !st.granted {
                spare += std::mem::take(&mut st.got);
            }
        }
        while spare > 0 {
            let Some(h) = Self::head(inner) else {
                break;
            };
            let mut st = h.lock().expect("waiter lock");
            let give = spare.min(st.n.saturating_sub(st.got));
            st.got += give;
            spare -= give;
            if st.got < st.n {
                break;
            }
            st.grant();
            drop(st);
            inner.queue.pop_front();
        }
        self.write(spare);
    }

    /// Whether the head of the queue is an `urgent` request.
    fn urgent_head(inner: &mut Inner) -> bool {
        Self::head(inner).is_some_and(|h| h.lock().expect("waiter lock").level == Priority::Urgent)
    }

    /// With no token free, gives `urgent` requests at the head of the queue
    /// tokens the headroom holds back, so repair work on the pool does not
    /// wait for a running job to end. Only held-back tokens are lent, so at
    /// most `size - limit` are in use beyond the limit; a lent token that
    /// comes back refills the held-back set like any other (see `reader`).
    fn lend(&self, inner: &mut Inner) {
        if inner.pause {
            return;
        }
        while inner.held_back > 0 && self.free() == 0 {
            let Some(h) = Self::head(inner) else {
                break;
            };
            let mut st = h.lock().expect("waiter lock");
            if st.level != Priority::Urgent {
                break;
            }
            let give = inner.held_back.min(st.n.saturating_sub(st.got));
            st.got += give;
            inner.held_back -= give;
            if st.got < st.n {
                break;
            }
            st.grant();
            drop(st);
            inner.queue.pop_front();
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

    /// Tokens missing from the pool while jobs run. Valid only when no
    /// process but the daemon has the FIFO open: then every token is in the
    /// FIFO, held back, granted to a running job (`granted`), or taken by a
    /// queued request. Anything short of that was read by a process that
    /// exited without writing it back. In-flight tokens (read by the reader
    /// thread, not yet booked) also show up here for a moment, so the caller
    /// acts only on a shortfall seen twice in a row. 0 while the pool is
    /// paused for a handover: tokens taken from queued requests are then in
    /// the daemon's hands and counted nowhere here.
    pub fn missing(&self, granted: usize) -> usize {
        let inner = self.inner.lock().expect("jobserver lock");
        if inner.pause {
            return 0;
        }
        let queued: usize = inner
            .queue
            .iter()
            .map(|w| w.lock().expect("waiter lock"))
            .filter(|w| !w.cancelled && !w.granted)
            .map(|w| w.got)
            .sum();
        let booked = inner.held_back + granted + queued;
        self.size.saturating_sub(booked + self.free())
    }

    /// Writes `n` lost tokens back.
    pub fn restore(&self, n: usize) {
        let _inner = self.inner.lock().expect("jobserver lock");
        self.write(n);
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
            // An urgent head gets the token before the held-back set does.
            if inner.held_back < inner.reserve && !Self::urgent_head(&mut inner) {
                inner.held_back += 1;
                continue;
            }
            match Self::head(&mut inner) {
                None => self.write(1),
                Some(h) => {
                    let mut st = h.lock().expect("waiter lock");
                    st.got += 1;
                    if st.got >= st.n {
                        st.grant();
                        drop(st);
                        inner.queue.pop_front();
                    }
                }
            }
            self.lend(&mut inner);
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
    async fn missing_counts_tokens_a_dead_client_took() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo");
        let p = Pool::new(&path, 6).unwrap();
        p.set_limit(4);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        assert_eq!(p.missing(1), 0, "held back 2, granted 1, free 3");
        {
            let mut client = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut buf = [0u8; 1];
            client.read_exact(&mut buf).unwrap();
        }
        assert_eq!(p.missing(1), 1, "the client exited holding one token");
        let mut b = p.acquire(3);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(b.held(), 2);
        assert_eq!(p.missing(1), 1, "a partly served request is booked");
        p.restore(1);
        assert!(
            ready_within(&mut b, 1000).await,
            "the restored token completes it"
        );
        assert_eq!(p.missing(4), 0);
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

    /// Releases one token and returns which of `ws` got it.
    async fn next_ready(p: &Pool, ws: &mut [(&str, &mut Waiter)]) -> String {
        p.release(1);
        for _ in 0..100 {
            for (name, w) in ws.iter_mut() {
                if w.held() == w.tokens() && w.take_held() > 0 {
                    return (*name).to_string();
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        "none".into()
    }

    #[tokio::test]
    async fn higher_levels_first_and_first_in_first_out_inside_a_level() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let mut l1 = p.acquire_at(1, Priority::Low);
        let mut n1 = p.acquire_at(1, Priority::Normal);
        let mut h1 = p.acquire_at(1, Priority::High);
        let mut n2 = p.acquire_at(1, Priority::Normal);
        let mut h2 = p.acquire_at(1, Priority::High);
        let mut order = Vec::new();
        let mut ws = [
            ("l1", &mut l1),
            ("n1", &mut n1),
            ("h1", &mut h1),
            ("n2", &mut n2),
            ("h2", &mut h2),
        ];
        for _ in 0..5 {
            order.push(next_ready(&p, &mut ws).await);
        }
        assert_eq!(order, ["h1", "h2", "n1", "n2", "l1"]);
    }

    #[tokio::test]
    async fn a_raised_request_goes_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let mut n = p.acquire_at(1, Priority::Normal);
        let mut l = p.acquire_at(1, Priority::Low);
        p.set_level(&l.handle(), Priority::High);
        let mut ws = [("n", &mut n), ("l", &mut l)];
        assert_eq!(next_ready(&p, &mut ws).await, "l");
        assert_eq!(next_ready(&p, &mut ws).await, "n");
    }

    /// A low request holding part of what it needs hands it to a high
    /// request that arrives: otherwise each would hold one of the two
    /// tokens and wait for the other forever.
    #[tokio::test]
    async fn a_new_head_takes_what_the_old_head_gathered() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let mut low = p.acquire_at(2, Priority::Low);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(low.held(), 1, "the free token went to the only waiter");
        let mut high = p.acquire_at(2, Priority::High);
        assert_eq!((low.held(), high.held()), (0, 1));
        p.release(a.take_held());
        assert!(ready_within(&mut high, 1000).await);
        assert!(!ready_within(&mut low, 200).await);
        p.release(high.take_held());
        assert!(ready_within(&mut low, 1000).await);
        assert_eq!(p.waiting_before(Priority::High), 0);
    }

    #[tokio::test]
    async fn requests_ahead_count_only_levels_served_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let _l = p.acquire_at(1, Priority::Low);
        let _n = p.acquire_at(1, Priority::Normal);
        let _h = p.acquire_at(1, Priority::High);
        assert_eq!(p.waiting_before(Priority::High), 1);
        assert_eq!(p.waiting_before(Priority::Normal), 2);
        assert_eq!(p.waiting_before(Priority::Low), 3);
    }

    /// Running jobs are never preempted: a granted request keeps its tokens
    /// when a higher request arrives, and a level change no longer moves it.
    #[tokio::test]
    async fn a_granted_request_keeps_its_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut low = p.acquire_at(2, Priority::Low);
        assert!(ready_within(&mut low, 1000).await);
        let mut high = p.acquire_at(1, Priority::High);
        p.set_level(&low.handle(), Priority::High);
        assert!(
            !ready_within(&mut high, 200).await,
            "took a running job's slot"
        );
        assert_eq!(low.held(), 2);
        p.release(low.take_held());
        assert!(ready_within(&mut high, 1000).await);
    }

    /// The head of the queue gathers tokens while later requests of its
    /// level or below, which would each fit in one free token, wait: as on
    /// a queue without levels.
    #[tokio::test]
    async fn a_large_request_is_not_overtaken_by_later_small_ones() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let mut big = p.acquire(2);
        let mut small = p.acquire(1);
        let mut low = p.acquire_at(1, Priority::Low);
        assert!(!ready_within(&mut small, 200).await, "overtook the head");
        assert!(!ready_within(&mut low, 50).await, "overtook the head");
        assert_eq!(big.held(), 1);
        p.release(a.take_held());
        assert!(ready_within(&mut big, 1000).await);
        p.release(big.take_held());
        assert!(ready_within(&mut small, 1000).await);
        assert!(ready_within(&mut low, 1000).await);
    }

    /// A request raised to `high` because it waited (what the daemon does
    /// after `--promote-after`) keeps its arrival order: it goes before
    /// every `high` request that came after it, so a stream of them cannot
    /// keep it waiting, even when it needs the whole pool.
    #[tokio::test]
    async fn a_promoted_request_goes_ahead_of_later_high_ones() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(2);
        assert!(ready_within(&mut a, 1000).await);
        let mut low = p.acquire_at(2, Priority::Low);
        let mut h1 = p.acquire_at(1, Priority::High);
        p.set_level(&low.handle(), Priority::High);
        let mut h2 = p.acquire_at(1, Priority::High);
        assert_eq!(p.waiting_before(Priority::High), 3);
        p.release(a.take_held());
        assert!(
            ready_within(&mut low, 1000).await,
            "the stream kept it waiting"
        );
        assert!(!ready_within(&mut h1, 200).await);
        assert!(!ready_within(&mut h2, 50).await);
        p.release(low.take_held());
        assert!(ready_within(&mut h1, 1000).await);
        assert!(ready_within(&mut h2, 1000).await);
    }

    /// Waits up to 1 s until the pool holds back `n` tokens.
    async fn held_back_reaches(p: &Pool, n: usize) -> bool {
        for _ in 0..100 {
            if p.held_back() == n {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// With no token free, `urgent` requests start on the tokens the
    /// headroom holds back, at most `size - limit` of them at a time, while
    /// a `normal` request keeps waiting; the tokens go back to the held-back
    /// set when the urgent jobs return them.
    #[tokio::test]
    async fn urgent_requests_start_on_held_back_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 4).unwrap();
        p.set_limit(2);
        assert!(held_back_reaches(&p, 2).await);
        let mut a = p.acquire(2);
        assert!(ready_within(&mut a, 1000).await);
        assert_eq!(p.free(), 0);
        let mut normal = p.acquire(1);
        let mut u1 = p.acquire_at(1, Priority::Urgent);
        assert!(ready_within(&mut u1, 200).await, "waited for a running job");
        let mut u2 = p.acquire_at(1, Priority::Urgent);
        assert!(ready_within(&mut u2, 200).await);
        assert_eq!(p.held_back(), 0);
        let mut u3 = p.acquire_at(1, Priority::Urgent);
        assert!(
            !ready_within(&mut u3, 200).await,
            "more than size - limit extra tokens"
        );
        assert!(!ready_within(&mut normal, 50).await);
        p.release(u1.take_held());
        assert!(
            ready_within(&mut u3, 1000).await,
            "the urgent head gets the token first"
        );
        assert!(!ready_within(&mut normal, 200).await);
        p.release(u2.take_held());
        p.release(u3.take_held());
        assert!(
            held_back_reaches(&p, 2).await,
            "tokens back in the held-back set"
        );
        assert!(
            !ready_within(&mut normal, 200).await,
            "normal still within the limit"
        );
        p.release(a.take_held());
        assert!(ready_within(&mut normal, 1000).await);
    }

    /// Without headroom there is nothing held back: an urgent request only
    /// goes to the head of the queue.
    #[tokio::test]
    async fn without_headroom_an_urgent_request_waits_at_the_head() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let mut high = p.acquire_at(1, Priority::High);
        let mut urgent = p.acquire_at(1, Priority::Urgent);
        assert!(!ready_within(&mut urgent, 200).await);
        assert_eq!(p.waiting_before(Priority::Urgent), 1);
        p.release(a.take_held());
        assert!(ready_within(&mut urgent, 1000).await);
        assert!(!ready_within(&mut high, 200).await);
    }
}
