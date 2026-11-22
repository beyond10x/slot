//! One GNU-make jobserver backed by a named FIFO.
//!
//! cargo, rustc and make read and write the FIFO directly. The daemon takes
//! tokens for jobs through one reader thread and serves the queue from the
//! tokens at hand (`Pool::distribute`). The queue is ordered by
//! [`Priority`], then inside a level by [`Place`], then by arrival (a
//! request that left the queue and comes back keeps its arrival number:
//! [`Pool::acquire_arrived`]).
//!
//! A request the tokens at hand do not cover gathers them, and while it may
//! no longer be passed ([`Place::passable`] false) no request behind it
//! takes one: a request for many tokens is then not overtaken by a stream of
//! small ones of its own level or below. While it may still be passed, a
//! later request that fits in the tokens at hand starts ahead of it, so a
//! request for the whole pool does not keep freed tokens idle while others
//! wait; and inside a level a short request ([`Place::short`]) stands ahead
//! of passable ones that are not short. The daemon makes every request
//! unpassable once it has waited its bound (`--pass-for`), and all of them
//! with a bound of zero, which is the order of level, then arrival. What
//! is gathered stays with one request, so two requests never each hold part
//! of what one of them needs. An `urgent` head with no token free borrows
//! the tokens the headroom holds back.

use std::collections::{HashMap, VecDeque};
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

/// A live process that has the pool's FIFO open, as [`holders`] read it
/// from `/proc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// Process id.
    pub pid: u32,
    /// Command name (`comm`).
    pub comm: String,
    /// Direct children it has not reaped: running ones and zombies.
    pub children: usize,
    /// Threads.
    pub threads: usize,
}

impl Holder {
    /// The most jobserver tokens the process can hold besides the one it was
    /// started with. The token watch counts this for every live holder, so
    /// it never writes back a token a live process has (`docs/design.md`
    /// § 21).
    ///
    /// A jobserver client holds one token for each unit of work it runs
    /// beyond the first; the first runs on the token the client was started
    /// with. What a unit is depends on the client:
    ///
    /// - A build driver (`cargo`, `make`, `ninja`) runs each unit as a
    ///   direct child, whatever the child is: a compiler, or a client of a
    ///   compile server with no compiler below it. Its threads are not
    ///   counted, because they mirror its children (`cargo` has 4, 5 and 7
    ///   threads with 1, 2 and 4 children).
    /// - Any other holder may run its units as threads: a `rustc` has no
    ///   child and holds a token for each codegen thread after its first.
    ///   The larger of its children and its threads counts.
    /// - A holder with more than one thread reads tokens on a helper
    ///   thread, and a token it has read is not a child or a worker yet for
    ///   a moment, so it counts one more. A holder with one thread starts
    ///   the child in the thread that read the token.
    pub fn may_hold(&self) -> usize {
        let units = if self.driver() {
            self.children
        } else {
            self.children.max(self.threads)
        };
        units.saturating_sub(1) + usize::from(self.threads > 1)
    }

    /// Whether the process runs every unit of its work as a child process.
    fn driver(&self) -> bool {
        matches!(self.comm.as_str(), "cargo" | "make" | "gmake" | "ninja")
    }
}

/// Command name, parent and thread count of one `/proc/<pid>/stat` line.
fn stat_fields(s: &str) -> Option<(String, u32, usize)> {
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    let comm = s.get(open + 1..close)?.to_string();
    // After the name: state, ppid (field 4), …, num_threads (field 20).
    let mut rest = s.get(close + 1..)?.split_whitespace();
    let ppid = rest.nth(1)?.parse().ok()?;
    let threads = rest.nth(15)?.parse().ok()?;
    Some((comm, ppid, threads))
}

/// Processes other than `own` that have the FIFO at `path` open, from
/// `/proc/<pid>/fd`, each with its children and threads from
/// `/proc/<pid>/stat` of every process. `None` when the FIFO or `/proc`
/// cannot be read: then nothing is known about who holds tokens. Processes
/// of other users are not readable and not counted; the FIFO is mode 0600,
/// so they cannot open it either. A descriptor counts by the FIFO's device
/// and inode, not its path: a sandboxed build opens it through a hard link
/// `slot gate` made (§ 19), and its `/proc` link names that path as the
/// sandbox sees it.
pub fn holders(path: &Path, own: u32) -> Option<Vec<Holder>> {
    use std::os::unix::fs::MetadataExt;
    let want = std::fs::metadata(path).map(|m| (m.dev(), m.ino())).ok()?;
    let procs = std::fs::read_dir("/proc").ok()?;
    let mut out: Vec<Holder> = Vec::new();
    let mut children: HashMap<u32, usize> = HashMap::new();
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let stat = std::fs::read_to_string(p.path().join("stat"))
            .ok()
            .and_then(|s| stat_fields(&s));
        if let Some((_, ppid, _)) = &stat {
            *children.entry(*ppid).or_default() += 1;
        }
        if pid == own {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(p.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .any(|fd| std::fs::metadata(fd.path()).is_ok_and(|m| (m.dev(), m.ino()) == want))
        {
            // A process whose `stat` is gone is exiting.
            let (comm, _, threads) = stat.unwrap_or_else(|| (String::new(), 0, 1));
            out.push(Holder {
                pid,
                comm,
                children: 0,
                threads,
            });
        }
    }
    for h in &mut out {
        h.children = children.get(&h.pid).copied().unwrap_or(0);
    }
    Some(out)
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
    /// Surplus tokens still to take out: the reader keeps the next that come
    /// back (see [`Pool::remove`]).
    debt: usize,
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

/// Where a request stands inside its level besides its arrival, and whether
/// later requests may start ahead of it. The daemon sets it from the job's
/// predicted run time and its wait (`server::priority::place`). The default,
/// neither short nor passable, is the order of priority, then arrival.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Place {
    /// Predicted to run a few seconds: it stands ahead of the requests of
    /// its level that are passable and not short.
    pub short: bool,
    /// Later requests may start ahead of it: a short one, or one that fits
    /// in the tokens at hand while this one cannot start. False once it has
    /// waited the bound: from then on no later request of its level or below
    /// starts before it.
    pub passable: bool,
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

    /// Tokens the request has gathered while it waits.
    pub fn held(&self) -> usize {
        self.0.lock().expect("waiter lock").got
    }

    /// Whether the request was moved ahead of one that the order of level,
    /// then arrival serves first.
    pub fn ahead(&self) -> bool {
        self.0.lock().expect("waiter lock").ahead
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
    place: Place,
    /// Moved ahead of a request that level, then arrival serves first:
    /// it stood or started before it. Kept once set.
    ahead: bool,
}

impl WaiterState {
    /// Where the request stands in the queue: level first; inside a level
    /// the requests that may no longer be passed, then the short passable
    /// ones, then the other passable ones; arrival last.
    fn rank(&self) -> (Priority, u8, u64) {
        let class = match (self.place.passable, self.place.short) {
            (false, _) => 0,
            (true, true) => 1,
            (true, false) => 2,
        };
        (self.level, class, self.seq)
    }

    /// Where plain priority, then arrival would put the request.
    fn plain(&self) -> (Priority, u64) {
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

    /// Whether the request was moved ahead of one that the order of level,
    /// then arrival serves first ([`WaiterHandle::ahead`]).
    pub fn ahead(&self) -> bool {
        self.state.lock().expect("waiter lock").ahead
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
        for w in &inner.queue {
            let mut st = w.lock().expect("waiter lock");
            st.n = st.n.min(limit);
        }
        self.distribute(&mut inner, 0);
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
        self.queue(n, level, None, Place::default())
    }

    /// Queues a request for `n` tokens (capped at the limit) at queue level
    /// `level` with the arrival number `arrival`: inside its level it stands
    /// ahead of every request with a higher number, also one queued before
    /// it. A job passes its id, so one that waited outside the queue (for
    /// memory, for its target directory, through an upgrade) comes back to
    /// the place its age gives it, not to the end.
    pub fn acquire_arrived(&self, n: usize, level: Priority, arrival: u64) -> Waiter {
        self.queue(n, level, Some(arrival), Place::default())
    }

    /// Like [`Pool::acquire_arrived`], with the request's [`Place`] inside
    /// its level.
    pub fn acquire_placed(&self, n: usize, level: Priority, arrival: u64, place: Place) -> Waiter {
        self.queue(n, level, Some(arrival), place)
    }

    fn queue(&self, n: usize, level: Priority, arrival: Option<u64>, place: Place) -> Waiter {
        let n = n.clamp(1, self.limit());
        let (tx, rx) = oneshot::channel();
        let mut inner = self.inner.lock().expect("jobserver lock");
        let seq = arrival.unwrap_or(inner.next_seq);
        inner.next_seq = inner.next_seq.max(seq + 1);
        let state = Arc::new(Mutex::new(WaiterState {
            n,
            got: 0,
            cancelled: false,
            granted: false,
            tx: Some(tx),
            level,
            seq,
            place,
            ahead: false,
        }));
        inner.queue.push_back(Arc::clone(&state));
        self.distribute(&mut inner, 0);
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
        let place = w.0.lock().expect("waiter lock").place;
        self.set_place(w, level, place);
    }

    /// Changes the level and the [`Place`] of a queued request, and serves
    /// the queue in its new order. A request already granted, or one whose
    /// level and place are unchanged, is left alone.
    pub fn set_place(&self, w: &WaiterHandle, level: Priority, place: Place) {
        let mut inner = self.inner.lock().expect("jobserver lock");
        {
            let mut st = w.0.lock().expect("waiter lock");
            if (st.level == level && st.place == place) || st.granted {
                return;
            }
            st.level = level;
            st.place = place;
        }
        self.distribute(&mut inner, 0);
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

    /// Serves the queue from the tokens at hand: every token the queued
    /// requests gathered, and `incoming` that the reader just took.
    ///
    /// The queue is put in order ([`WaiterState::rank`]) and walked from
    /// the front. A request that the tokens at hand cover is granted. One
    /// they do not cover stops the walk when it may no longer be passed:
    /// nobody behind it takes a token before it. While it may be passed,
    /// the walk goes on, and a later request that fits in what is at hand
    /// starts ahead of it. What is left stays booked with one request (the
    /// one that stopped the walk, else the first not covered), so no two
    /// requests each hold part of what one of them needs; with nobody
    /// waiting it goes back to the FIFO.
    fn distribute(&self, inner: &mut Inner, incoming: usize) {
        inner
            .queue
            .retain(|w| !w.lock().expect("waiter lock").cancelled);
        inner
            .queue
            .make_contiguous()
            .sort_by_cached_key(|w| w.lock().expect("waiter lock").rank());
        let mut hand = incoming;
        for w in &inner.queue {
            hand += std::mem::take(&mut w.lock().expect("waiter lock").got);
        }
        let mut keeper: Option<usize> = None;
        let mut granted: Vec<usize> = Vec::new();
        for (i, w) in inner.queue.iter().enumerate() {
            let st = w.lock().expect("waiter lock");
            if hand >= st.n {
                hand -= st.n;
                granted.push(i);
                continue;
            }
            if keeper.is_none() || !st.place.passable {
                keeper = Some(i);
            }
            if !st.place.passable {
                break;
            }
        }
        // Moved ahead: a request that starts, or stands, before one that
        // priority, then arrival serves first and that still waits.
        let waiting: Vec<(Priority, u64)> = inner
            .queue
            .iter()
            .enumerate()
            .filter(|(i, _)| !granted.contains(i))
            .map(|(_, w)| w.lock().expect("waiter lock").plain())
            .collect();
        let first_waiting = waiting.iter().min().copied();
        let mut behind: Option<(Priority, u64)> = None;
        for (i, w) in inner.queue.iter().enumerate().rev() {
            let mut st = w.lock().expect("waiter lock");
            let me = st.plain();
            if granted.contains(&i) {
                st.ahead |= first_waiting.is_some_and(|f| f < me);
                st.got = st.n;
                st.grant();
            } else {
                st.ahead |= behind.is_some_and(|b| b < me);
                behind = Some(behind.map_or(me, |b| b.min(me)));
            }
        }
        match keeper {
            Some(k) => inner.queue[k].lock().expect("waiter lock").got = hand,
            None => self.write(hand),
        }
        let mut i = 0;
        inner.queue.retain(|_| {
            i += 1;
            !granted.contains(&(i - 1))
        });
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
        let mut inner = self.inner.lock().expect("jobserver lock");
        if !inner.queue.is_empty() {
            return 0;
        }
        // The FIFO is set to its size here, so nothing is left to take out.
        inner.debt = 0;
        let diff = (self.size - inner.held_back) as i64 - self.free() as i64;
        if diff > 0 {
            self.write(diff as usize);
            return diff;
        }
        if diff < 0 {
            return -(self.take((-diff) as usize) as i64);
        }
        diff
    }

    /// Takes up to `n` tokens out of the FIFO without blocking: what is
    /// there now.
    fn take(&self, n: usize) -> usize {
        let mut buf = vec![0u8; n];
        let mut taken = 0;
        while taken < buf.len() {
            match (&self.file).read(&mut buf[taken..]) {
                Ok(0) | Err(_) => break,
                Ok(k) => taken += k,
            }
        }
        taken
    }

    /// Tokens the daemon has booked, and those in the FIFO: held back,
    /// granted to running jobs (`granted`), gathered by a queued request,
    /// or free.
    fn placed(&self, inner: &Inner, granted: usize) -> usize {
        let queued: usize = inner
            .queue
            .iter()
            .map(|w| w.lock().expect("waiter lock"))
            .filter(|w| !w.cancelled && !w.granted)
            .map(|w| w.got)
            .sum();
        inner.held_back + granted + queued + self.free()
    }

    /// Tokens that are neither in the FIFO nor booked by the daemon while
    /// jobs run: not held back, not granted to a running job (`granted`),
    /// not gathered by a queued request. A live process with the FIFO open
    /// has them, or a process that exited without writing them back; the
    /// caller tells the two apart ([`Holder::may_hold`]). In-flight tokens
    /// (read by the reader thread, not yet booked) also show up here for a
    /// moment, so the caller acts only on a shortfall seen twice in a row.
    /// 0 while the pool is paused for a handover: tokens taken from queued
    /// requests are then in the daemon's hands and counted nowhere here.
    pub fn missing(&self, granted: usize) -> usize {
        let inner = self.inner.lock().expect("jobserver lock");
        if inner.pause {
            return 0;
        }
        (self.size + inner.debt).saturating_sub(self.placed(&inner, granted))
    }

    /// Tokens in circulation beyond the pool size while jobs run: a slot
    /// released twice, or a token written back that a live process still
    /// held. What live processes hold only adds to it, so the count is a
    /// lower bound and needs no look at them. Tokens [`Pool::remove`] still
    /// has to take out are not counted again. 0 while the pool is paused.
    pub fn surplus(&self, granted: usize) -> usize {
        let inner = self.inner.lock().expect("jobserver lock");
        if inner.pause {
            return 0;
        }
        self.placed(&inner, granted)
            .saturating_sub(self.size + inner.debt)
    }

    /// Writes `n` lost tokens back.
    pub fn restore(&self, n: usize) {
        let _inner = self.inner.lock().expect("jobserver lock");
        self.write(n);
    }

    /// Takes `n` surplus tokens out of circulation: those in the FIFO at
    /// once, and for the rest the next that come back, before the headroom
    /// or a queued request gets them. Returns how many it took at once.
    pub fn remove(&self, n: usize) -> usize {
        let mut inner = self.inner.lock().expect("jobserver lock");
        let taken = self.take(n);
        inner.debt += n - taken;
        drop(inner);
        self.wake.notify_all();
        taken
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
                        if Self::head(&mut inner).is_some()
                            || inner.held_back < inner.reserve
                            || inner.debt > 0
                        {
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
            // A surplus token goes nowhere.
            if inner.debt > 0 {
                inner.debt -= 1;
                continue;
            }
            // An urgent head gets the token before the held-back set does.
            if inner.held_back < inner.reserve && !Self::urgent_head(&mut inner) {
                inner.held_back += 1;
                continue;
            }
            self.distribute(&mut inner, 1);
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

    /// A request that comes back with its arrival number stands where its
    /// age puts it: ahead of later arrivals of its level that queued while
    /// it was away, behind every higher level.
    #[tokio::test]
    async fn a_returning_request_keeps_the_place_of_its_arrival() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let mut n7 = p.acquire_arrived(1, Priority::Normal, 7);
        let mut n9 = p.acquire_arrived(1, Priority::Normal, 9);
        let mut h8 = p.acquire_arrived(1, Priority::High, 8);
        let mut n3 = p.acquire_arrived(1, Priority::Normal, 3);
        let mut later = p.acquire_at(1, Priority::Normal);
        let mut ws = [
            ("n7", &mut n7),
            ("n9", &mut n9),
            ("h8", &mut h8),
            ("n3", &mut n3),
            ("later", &mut later),
        ];
        let mut order = Vec::new();
        for _ in 0..5 {
            order.push(next_ready(&p, &mut ws).await);
        }
        assert_eq!(order, ["h8", "n3", "n7", "n9", "later"]);
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

    /// With passing off (every request at the default [`Place`], what the
    /// daemon does with `--pass-for 0`) the head of the queue gathers tokens
    /// while later requests of its level or below, which would each fit in
    /// one free token, wait: as on a queue without levels. With passing on
    /// this no longer holds:
    /// `a_whole_pool_request_lets_later_small_ones_start_while_it_may_be_passed`.
    #[tokio::test]
    async fn with_passing_off_a_large_request_is_not_overtaken_by_later_small_ones() {
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
    /// Polls `cond` for up to 5 s.
    async fn reaches(mut cond: impl FnMut() -> bool) -> bool {
        for _ in 0..500 {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    fn holder(comm: &str, children: usize, threads: usize) -> Holder {
        Holder {
            pid: 1,
            comm: comm.into(),
            children,
            threads,
        }
    }

    #[test]
    fn a_holder_counts_for_the_most_tokens_it_can_hold() {
        for (comm, children, threads, most, why) in [
            ("cargo", 4, 7, 4, "holds 3 for 4 children, 1 on its way"),
            (
                "cargo",
                2,
                5,
                2,
                "children that are clients of a compile server count",
            ),
            ("cargo", 1, 1, 0, "waits on one child, reads no token"),
            ("cargo", 0, 3, 1, "a token read before the first child"),
            ("rustc", 0, 9, 9, "a token for each codegen thread"),
            ("make", 3, 1, 2, "one thread: no token on its way"),
            ("sh", 2, 1, 1, "any client with one thread"),
            ("sh", 0, 1, 0, "no work, no token"),
            ("build-script-bu", 2, 6, 6, "units may be threads"),
        ] {
            assert_eq!(
                holder(comm, children, threads).may_hold(),
                most,
                "{comm} with {children} children and {threads} threads: {why}"
            );
        }
    }

    #[test]
    fn stat_fields_reads_name_parent_and_threads() {
        let line = "4242 (a b) c) S 77 4242 4242 0 -1 4194304 1 2 3 4 5 6 7 8 20 0 9 0 100 0 0";
        assert_eq!(stat_fields(line), Some(("a b) c".into(), 77, 9)));
        assert_eq!(stat_fields("4242 (short) S 77"), None);
    }

    #[tokio::test]
    async fn holders_lists_a_process_with_its_children_and_threads() {
        use std::os::unix::process::CommandExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo");
        let _p = Pool::new(&path, 2).unwrap();
        let own = std::process::id();
        assert_eq!(holders(&dir.path().join("none"), own), None);
        let ready = dir.path().join("ready");
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("exec 3<>\"$FIFO\"; sleep 600 3>&- & sleep 600 3>&- & : > \"$READY\"; wait")
            .env("FIFO", &path)
            .env("READY", &ready)
            .stdin(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        // A child that has not started its command yet still has the FIFO
        // open, so the list is read until only the client is on it.
        let mut found = None;
        let started = reaches(|| {
            found = ready.exists().then(|| holders(&path, own)).flatten();
            found
                .as_ref()
                .is_some_and(|f| f.len() == 1 && f[0].children == 2)
        })
        .await;
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(child.id() as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
        let _ = child.wait();
        assert!(
            started,
            "the client and its two children were not listed: {found:?}"
        );
        let found = found.expect("the FIFO is there");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            (found[0].pid, found[0].children, found[0].threads),
            (child.id(), 2, 1),
            "{found:?}"
        );
        assert_eq!(found[0].may_hold(), 1);
    }

    #[tokio::test]
    async fn remove_takes_free_tokens_and_then_the_next_that_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(2);
        assert!(ready_within(&mut a, 1000).await);
        assert_eq!((p.surplus(2), p.missing(2)), (0, 0));
        // A slot released twice: the third token is free.
        p.release(1);
        assert_eq!((p.surplus(2), p.missing(2)), (1, 0));
        assert_eq!(p.remove(1), 1, "taken out of the FIFO");
        assert_eq!((p.free(), p.surplus(2)), (0, 0));
        // Again, and this time a queued request takes it.
        let mut b = p.acquire(2);
        p.release(1);
        assert!(reaches(|| b.held() == 1).await);
        assert_eq!((p.free(), p.surplus(2)), (0, 1));
        assert_eq!(p.remove(1), 0, "none is free");
        assert_eq!(
            (p.surplus(2), p.missing(2)),
            (0, 0),
            "a token still to take out is not counted again"
        );
        p.release(a.take_held());
        assert!(ready_within(&mut b, 1000).await);
        assert_eq!(
            (p.free(), p.surplus(2), p.missing(2)),
            (0, 0, 0),
            "one of the two went to the request, the other out"
        );
        p.release(b.take_held());
        assert!(reaches(|| p.free() == 2).await);
        assert_eq!(p.reconcile(), 0);
    }

    const PASS: Place = Place {
        short: false,
        passable: true,
    };
    const SHORT: Place = Place {
        short: true,
        passable: true,
    };
    const FIXED: Place = Place {
        short: false,
        passable: false,
    };

    /// A request for every slot that may still be passed keeps no token from
    /// later requests that fit: with 1 of 4 slots taken, three 1-slot
    /// requests behind a 4-slot one start at once, each marked as moved
    /// ahead. The large one gathers the tokens again as they come back, and
    /// they stay booked while it holds them.
    #[tokio::test]
    async fn a_whole_pool_request_lets_later_small_ones_start_while_it_may_be_passed() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 4).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let mut big = p.acquire_placed(4, Priority::Normal, 10, PASS);
        assert!(reaches(|| big.held() == 3).await, "gathers the free tokens");
        assert_eq!(p.missing(1), 0, "gathered tokens are booked");
        let mut small: Vec<Waiter> = (11..14)
            .map(|i| p.acquire_placed(1, Priority::Normal, i, PASS))
            .collect();
        for (i, w) in small.iter_mut().enumerate() {
            assert!(
                ready_within(w, 1000).await,
                "1-slot request {i} waited behind the whole-pool request"
            );
            assert!(w.ahead(), "request {i} is not marked as moved ahead");
        }
        assert_eq!(big.held(), 0);
        assert!(!big.ahead());
        for w in &small {
            p.release(w.take_held());
        }
        assert!(reaches(|| big.held() == 3).await, "gathers them again");
        assert_eq!(p.missing(1), 0);
        p.release(a.take_held());
        assert!(ready_within(&mut big, 1000).await);
        p.release(big.take_held());
        assert!(reaches(|| p.free() == 4).await);
    }

    /// Once the large request may no longer be passed (it has waited the
    /// bound), no later request starts ahead of it, short or not: it gathers
    /// every token that comes back and starts when it has all.
    #[tokio::test]
    async fn once_a_whole_pool_request_has_waited_the_bound_nothing_starts_ahead_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 2).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        let mut big = p.acquire_placed(2, Priority::Normal, 10, PASS);
        assert!(reaches(|| big.held() == 1).await);
        let mut s1 = p.acquire_placed(1, Priority::Normal, 11, PASS);
        assert!(ready_within(&mut s1, 1000).await, "passed while it may be");
        p.set_place(&big.handle(), Priority::Normal, FIXED);
        let mut s2 = p.acquire_placed(1, Priority::Normal, 12, PASS);
        let mut s3 = p.acquire_placed(1, Priority::Normal, 13, SHORT);
        p.release(s1.take_held());
        assert!(reaches(|| big.held() == 1).await, "the token went past it");
        assert!(!ready_within(&mut s2, 200).await, "started ahead of it");
        assert!(
            !ready_within(&mut s3, 50).await,
            "a short one started ahead"
        );
        p.release(a.take_held());
        assert!(ready_within(&mut big, 1000).await);
        p.release(big.take_held());
        assert!(ready_within(&mut s3, 1000).await);
        assert!(ready_within(&mut s2, 1000).await);
    }

    /// Inside a level a request predicted short starts before a passable one
    /// that is not, although it came later, and is marked as moved ahead.
    #[tokio::test]
    async fn a_short_request_starts_ahead_of_a_longer_one_of_its_level() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let mut long = p.acquire_placed(1, Priority::Normal, 10, PASS);
        let mut short = p.acquire_placed(1, Priority::Normal, 11, SHORT);
        assert_eq!((short.ahead(), long.ahead()), (true, false));
        let mut ws = [("long", &mut long), ("short", &mut short)];
        assert_eq!(next_ready(&p, &mut ws).await, "short");
        assert_eq!(next_ready(&p, &mut ws).await, "long");
    }

    /// A stream of short requests passes a longer one only while it may be
    /// passed: once it has waited the bound it starts before every request
    /// that came after it.
    #[tokio::test]
    async fn a_stream_of_short_requests_does_not_hold_a_request_past_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let mut long = p.acquire_placed(1, Priority::Normal, 10, PASS);
        for i in 11..14 {
            let mut s = p.acquire_placed(1, Priority::Normal, i, SHORT);
            let mut ws = [("long", &mut long), ("short", &mut s)];
            assert_eq!(next_ready(&p, &mut ws).await, "short", "short {i}");
        }
        p.set_place(&long.handle(), Priority::Normal, FIXED);
        let mut s = p.acquire_placed(1, Priority::Normal, 20, SHORT);
        assert!(!s.ahead());
        let mut ws = [("long", &mut long), ("short", &mut s)];
        assert_eq!(next_ready(&p, &mut ws).await, "long");
        assert_eq!(next_ready(&p, &mut ws).await, "short");
    }

    /// The levels still come first: a short request of a lower level does
    /// not pass a request of a higher level.
    #[tokio::test]
    async fn a_short_request_does_not_pass_a_request_of_a_higher_level() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let mut high = p.acquire_placed(1, Priority::High, 10, PASS);
        let mut short = p.acquire_placed(1, Priority::Normal, 11, SHORT);
        let mut ws = [("short", &mut short), ("high", &mut high)];
        assert_eq!(next_ready(&p, &mut ws).await, "high");
        assert_eq!(next_ready(&p, &mut ws).await, "short");
    }

    /// With passing off a request predicted short waits its turn: priority,
    /// then arrival.
    #[tokio::test]
    async fn with_passing_off_a_short_request_waits_its_turn() {
        let dir = tempfile::tempdir().unwrap();
        let p = Pool::new(&dir.path().join("fifo"), 1).unwrap();
        let mut a = p.acquire(1);
        assert!(ready_within(&mut a, 1000).await);
        a.take_held();
        let off = Place {
            short: true,
            passable: false,
        };
        let mut long = p.acquire_placed(1, Priority::Normal, 10, Place::default());
        let mut short = p.acquire_placed(1, Priority::Normal, 11, off);
        assert!(!short.ahead());
        let mut ws = [("short", &mut short), ("long", &mut long)];
        assert_eq!(next_ready(&p, &mut ws).await, "long");
        assert_eq!(next_ready(&p, &mut ws).await, "short");
    }
}
