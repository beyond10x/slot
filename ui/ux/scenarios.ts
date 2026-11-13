// Hermetic scenarios for the UX gates: every response and event the page gets, state by state.
import type { Chunk, Event, HostCensus, Job, LiveJob, State } from '../src/generated/types.ts';
import { POOL_BASE } from '../src/lib/headroom.ts';
import type { MachineNumbers } from '../src/lib/headroom.ts';

export type Tick = Extract<Event, { type: 'tick' }>;

/** One state change: the events pushed on `api/events`, then the clock moves `dt_s` seconds. */
export interface Step {
  moment: string;
  dt_s: number;
  events: Event[];
}

/** What the screen-only user is asked to do, and the truth it is held to. */
export type Task =
  | { kind: 'open-output'; job: number; expect: string[] }
  | { kind: 'set-headroom'; percent: number; expect: string; fails?: boolean }
  | { kind: 'queue-position'; job: number; position: number }
  /** Read a state off the screen: `expect` is visible in the viewport. */
  | { kind: 'see'; expect: string };

export interface Scenario {
  name: string;
  /** Anchor scenarios run every round so rounds compare; the rest rotate. */
  anchor: boolean;
  card: string;
  state: State;
  history: Tick[];
  steps: Step[];
  output: Record<number, { chunks: Chunk[]; end: string }>;
  task: Task | null;
}

export const T0 = Date.UTC(2026, 10, 12, 9, 0, 0);
const s = (sec: number): number => T0 + sec * 1000;

const SESSIONS = [
  ['0b1f9e2a-6c1d-4d2a-9f5e-1a2b3c4d5e6f', 'remote build server'],
  ['7c2e1d4f-8a9b-4c3d-b2e1-0f9e8d7c6b5a', 'checkout service: payment retries'],
  ['3d4c5b6a-7980-4a1b-8c2d-3e4f5a6b7c8d', 'docs site redesign'],
  ['9e8d7c6b-5a49-4382-a716-0504f3e2d1c0', ''],
  ['1a2b3c4d-5e6f-4071-8293-a4b5c6d7e8f9', 'language server: incremental parsing of very long files with many macros'],
] as const;

const REPOS = ['ledger', 'checkout', 'docs-site', 'parser', 'scheduler-with-a-long-repository-name'];

const CMDS: Array<[string, string[]]> = [
  ['rust', ['cargo', 'build', '--workspace', '--all-targets']],
  ['rust', ['cargo', 'test', '-p', 'ledger-core', '--', '--test-threads', '4']],
  ['go', ['go', 'test', './...', '-run', 'TestCheckout', '-count=1']],
  ['node-test', ['npx', 'playwright', 'test', '--workers=2', 'e2e/checkout.spec.ts']],
  ['rust', ['cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings']],
  ['generic', ['task', 'check']],
];

function job(id: number, over: Partial<Job> = {}): Job {
  const [toolchain, argv] = CMDS[id % CMDS.length];
  const [session] = SESSIONS[id % SESSIONS.length];
  return {
    id, argv, cwd: `/work/${REPOS[id % REPOS.length]}`, repo: REPOS[id % REPOS.length], session,
    client: 'ws:1', toolchain, remote: false, tokens: toolchain === 'rust' ? 4 : 2, state: 'running',
    enqueued_ms: s(-400 + id), started_ms: s(-380 + id), ended_ms: 0, exit: 0, cpu_s: 0,
    compile_server_cpu_s: 0, peak_rss: 0, wait_reason: '', target_dir: '', ...over,
  };
}

function done(id: number, endSec: number, over: Partial<Job> = {}): Job {
  return job(id, {
    state: 'ok', enqueued_ms: s(endSec - 90), started_ms: s(endSec - 80), ended_ms: s(endSec),
    cpu_s: 300 + id * 7, peak_rss: (400 + id * 13) * 2 ** 20, ...over,
  });
}

function queued(id: number, sinceSec: number): Job {
  return job(id, { state: 'queued', enqueued_ms: s(sinceSec), started_ms: 0 });
}

function liveOf(j: Job, k: number): LiveJob {
  return {
    id: j.id, cores: Math.round((0.7 + 0.5 * (1 + Math.sin(j.id * 1.7 + k / 9))) * 10) / 10, rss: (600 + j.id * 11) * 2 ** 20, procs: 3 + (j.id % 4),
    units: [
      { kind: 'compile', name: `crate_${j.id}_a`, cores: 1.2, secs: 4 },
      { kind: 'link', name: `bin_${j.id}`, cores: 0.9, secs: 2 },
    ],
    units_seen: 40 + j.id,
  };
}

function census(running: Job[], k: number, sessions = SESSIONS.length): HostCensus {
  const live = running.map((j) => liveOf(j, k));
  const build = live.reduce((a, l) => a + l.cores, 0);
  return {
    host_cores: build + 2.5, build_cores: build + 1.1, unmanaged_cores: 1.1, psi_cpu: 12 + (k % 3),
    psi_memory: 0.5, psi_io: 1.2, load1: 9.5,
    classes: [
      { class: 'rust-compile', managed: true, cores: build * 0.7, rss: 3 * 2 ** 30, procs: 12 },
      { class: 'go-test', managed: true, cores: build * 0.3, rss: 2 ** 30, procs: 6 },
      { class: 'node-test', managed: false, cores: 1.1, rss: 2 ** 29, procs: 3 },
    ],
    sessions: SESSIONS.slice(0, sessions).map(([session, title], i) => ({
      session, title, cores: 4 - i * 0.6, unmanaged_cores: i === 3 ? 1.1 : 0, rss: (2 - i * 0.2) * 2 ** 30,
      procs: 8 - i,
    })),
    recommended_tokens: 18, unmanaged_smoothed: 1.0,
  };
}

function tick(
  t: number, running: Job[], tokens: number, limit: number, headroom: number, waiting: number, k: number, callApps: string[] = [],
): Tick {
  const used = running.reduce((a, j) => a + j.tokens, 0);
  const live = running.map((j) => liveOf(j, k));
  return {
    type: 'tick', t, tokens, limit, headroom_percent: headroom, call_apps: callApps, free: Math.max(0, limit - used), waiting,
    sys_cpu: 0.55 + (k % 4) * 0.05, job_cores: live.reduce((a, l) => a + l.cores, 0), jobs: live,
    census: census(running, k), upgrade: '', cpu_quota_percent: headroom ? (100 - headroom) * 20 : null, ...POOL_BASE,
  };
}

function history(running: Job[], tokens: number, limit: number, headroom: number, waiting: number, callApps: string[] = []): Tick[] {
  const out: Tick[] = [];
  for (let k = 0; k < 120; k++) out.push(tick(s(-120 + k), running, tokens, limit, headroom, waiting, k, callApps));
  return out;
}

function state(active: Job[], doneJobs: Job[], tokens: number, headroom: number, callApps: string[] = []): State {
  const limit = Math.max(1, Math.round((tokens * (100 - headroom)) / 100));
  const used = active.filter((j) => j.state === 'running').reduce((a, j) => a + j.tokens, 0);
  return {
    host: 'workstation', cores: 20, tokens, limit, headroom_percent: headroom,
    cpu_quota_percent: headroom ? (100 - headroom) * 20 : null, free: Math.max(0, limit - used), active,
    done: doneJobs, now_ms: T0, toolchains: ['rust', 'go', 'node-test', 'generic'], call_apps: callApps,
    sccache: { enabled: true, servers: 1, short: 0, refills: 0, tokens: 0, last_refill_ms: 0 }, upgrade: '',
    evict: { floor: 0, dry_run: true, known: 0, free: 0, checked_ms: 0, last: [], deleted_bytes: 0 }, ...POOL_BASE,
  };
}

const COMPILE_ERROR: Chunk[] = [
  { t: 'err', b: '   Compiling ledger-core v0.4.0 (/work/ledger/crates/core)\n' },
  { t: 'err', b: '\x1b[1m\x1b[31merror[E0308]\x1b[0m: mismatched types\n  --> crates/core/src/post.rs:88:21\n' },
  { t: 'err', b: '   |\n88 |     let amount: u64 = entry.amount;\n   |                 ---   ^^^^^^^^^^^^ expected `u64`, found `i64`\n' },
  { t: 'err', b: 'error: could not compile `ledger-core` (lib) due to 1 previous error\n' },
];

function busyFailed(): Scenario {
  const running = [job(40), job(41), job(43), job(44), job(45)];
  const waiting = [queued(46, -40), queued(47, -25), queued(48, -10)];
  const finished = Array.from({ length: 12 }, (_, i) => done(28 + i, -300 + i * 20));
  finished[9] = done(37, -120, { state: 'failed', exit: 101 });
  const failed42 = job(42, {
    repo: 'ledger', cwd: '/work/ledger', argv: ['cargo', 'build', '-p', 'ledger-core'], toolchain: 'rust', tokens: 4,
    state: 'failed', exit: 101, ended_ms: s(4), cpu_s: 210, peak_rss: 900 * 2 ** 20,
  });
  const st = state([...running, ...waiting], finished, 20, 0);
  const steps: Step[] = [
    { moment: 'loaded', dt_s: 1, events: [tick(s(1), running, 20, 20, 0, 3, 1)] },
    {
      moment: 'build failed', dt_s: 3,
      events: [{ type: 'job', job: { ...failed42, started_ms: s(-300) } }, tick(s(4), running, 20, 20, 0, 3, 2)],
    },
    {
      moment: 'queued job starts', dt_s: 2,
      events: [
        { type: 'job', job: { ...waiting[0], state: 'running', started_ms: s(5) } },
        tick(s(6), [...running, { ...waiting[0], state: 'running' }], 20, 20, 0, 2, 3),
      ],
    },
  ];
  return {
    name: 'busy-failed', anchor: true,
    card: 'An agent says its build failed a moment ago. Open that build\'s output and read the error.',
    state: st, history: history(running, 20, 20, 0, 3), steps,
    output: { 42: { chunks: COMPILE_ERROR, end: 'exit 101' } },
    task: { kind: 'open-output', job: 42, expect: ['error[E0308]', 'exit 101'] },
  };
}

function videoCall(name: string, fails: boolean): Scenario {
  const running = [job(60), job(61), job(62), job(63), job(64), job(65)];
  const finished = Array.from({ length: 8 }, (_, i) => done(50 + i, -200 + i * 20));
  const st = state(running, finished, 20, 0);
  return {
    name, anchor: !fails,
    card: fails
      ? 'You join a video call and keep 40 % of the CPU for yourself, but the daemon is older than the page.'
      : 'You join a video call: keep 40 % of the CPU for yourself, and see what builds get now.',
    state: st, history: history(running, 20, 20, 0, 0),
    steps: [{ moment: 'loaded', dt_s: 1, events: [tick(s(1), running, 20, 20, 0, 0, 1)] }],
    output: {},
    task: fails
      ? { kind: 'set-headroom', percent: 40, expect: 'older than this page', fails: true }
      : { kind: 'set-headroom', percent: 40, expect: 'builds may use 12 of 20 cores, 12 of 20 slots' },
  };
}

function queuePosition(): Scenario {
  const running = [job(70), job(71), job(72), job(73), job(74), job(75)];
  const waiting = [queued(76, -90), queued(77, -70), queued(78, -50), queued(79, -30), queued(80, -12), queued(81, -3)];
  const finished = Array.from({ length: 10 }, (_, i) => done(58 + i, -400 + i * 25));
  const st = state([...running, ...waiting], finished, 20, 0);
  return {
    name: 'queue-position', anchor: false,
    card: 'An agent asks why its build (job 79) has not started. Find how many jobs are ahead of it.',
    state: st, history: history(running, 20, 20, 0, 6),
    steps: [{ moment: 'loaded', dt_s: 1, events: [tick(s(1), running, 20, 20, 0, 6, 1)] }],
    output: {}, task: { kind: 'queue-position', job: 79, position: 4 },
  };
}

function crowded(): Scenario {
  const longArgv = [
    'cargo', 'test', '--workspace', '--all-targets', '--features', 'postgres,tls-rustls,metrics,tracing-subscriber',
    '--', '--test-threads', '4', '--nocapture',
  ];
  const running = Array.from({ length: 14 }, (_, i) => job(100 + i, i % 3 === 0 ? { argv: longArgv } : {}));
  const waiting = Array.from({ length: 10 }, (_, i) => queued(114 + i, -100 + i * 9));
  const finished = Array.from({ length: 40 }, (_, i) => done(60 + i, -900 + i * 20, i % 7 === 0 ? { state: 'failed', exit: 1 } : {}));
  const st = state([...running, ...waiting], finished, 20, 30);
  const long: Chunk[] = Array.from({ length: 80 }, (_, i) => ({
    t: i % 9 === 0 ? 'err' : 'out',
    b: `test ledger::post::tests::case_${i}_with_a_long_name_that_runs_past_the_pane ... ok ${'.'.repeat(i % 40)}\n`,
  }));
  return {
    name: 'crowded', anchor: false,
    card: 'A busy afternoon: 14 builds, 10 queued, long commands and session titles, output pane open.',
    state: st, history: history(running, 20, 14, 30, 10),
    steps: [
      { moment: 'loaded', dt_s: 1, events: [tick(s(1), running, 20, 14, 30, 10, 1)] },
      { moment: 'next tick', dt_s: 1, events: [tick(s(2), running, 20, 14, 30, 10, 2)] },
    ],
    output: { 100: { chunks: long, end: '' } },
    task: null,
  };
}

function callMode(): Scenario {
  const apps = ['browser'];
  const running = [job(130), job(131), job(132), job(133)];
  const waiting = [
    { ...queued(134, -60), wait_reason: 'free space on the build filesystem is below 25 GB' },
    { ...queued(135, -20), wait_reason: 'available memory is below what the job needs' },
  ];
  const finished = Array.from({ length: 6 }, (_, i) => done(120 + i, -300 + i * 30));
  const st = state([...running, ...waiting], finished, 20, 40, apps);
  return {
    name: 'call-mode', anchor: false,
    card: 'A call holds the headroom at 40 %; two builds wait for disk space and memory, not for slots.',
    state: st, history: history(running, 20, 12, 40, 2, apps),
    steps: [{ moment: 'loaded', dt_s: 1, events: [tick(s(1), running, 20, 12, 40, 2, 1, apps)] }],
    output: {}, task: null,
  };
}

function upgradePending(): Scenario {
  const running = [job(140), job(141), job(142)];
  const waiting = [queued(143, -40), queued(144, -30), queued(145, -12)];
  const finished = Array.from({ length: 5 }, (_, i) => done(130 + i, -300 + i * 40));
  const line = 'upgrade pending: no new job starts for 42s, re-executing once none runs';
  const st = { ...state([...running, ...waiting], finished, 20, 0), upgrade: line };
  const t = (sec: number, k: number) => ({ ...tick(s(sec), running, 20, 20, 0, 3, k), upgrade: line });
  return {
    name: 'upgrade-pending', anchor: false,
    card: 'Three builds wait although slots are free. Find out why.',
    state: st, history: history(running, 20, 20, 0, 3).map((h) => ({ ...h, upgrade: line })),
    steps: [{ moment: 'loaded', dt_s: 1, events: [t(1, 1)] }, { moment: 'still held', dt_s: 1, events: [t(2, 2)] }],
    output: {}, task: { kind: 'see', expect: 'upgrade pending' },
  };
}

function idle(): Scenario {
  const finished = Array.from({ length: 3 }, (_, i) => done(1 + i, -600 + i * 60));
  const st = state([], finished, 20, 0);
  return {
    name: 'idle', anchor: false, card: 'Nothing builds. The page must say so.',
    state: st, history: history([], 20, 20, 0, 0),
    steps: [{ moment: 'loaded', dt_s: 1, events: [tick(s(1), [], 20, 20, 0, 0, 1)] }],
    output: {}, task: null,
  };
}

function machineHeadroom(): Scenario {
  const running = [job(150), job(151), job(152), job(153)];
  const waiting = [queued(154, -30)];
  const finished = Array.from({ length: 5 }, (_, i) => done(140 + i, -300 + i * 40));
  const machine = (k: number): MachineNumbers & { cpu_quota_percent: number } => ({
    headroom_base: 'machine', busy_cores: 18.2 + (k % 3) * 0.1, target_cores: 11, outside_cores: 9.1, quota_floor: true, over_target: true,
    outside: [
      { name: 'session-3.scope', cores: 5.9 }, { name: 'docker 3f2a1b2c3d4e', cores: 3.2 },
      { name: 'system.slice', cores: 0.4 }, { name: 'app-editor.scope', cores: 0.2 },
    ],
    cpu_quota_percent: 200,
  });
  const st = { ...state([...running, ...waiting], finished, 20, 45), ...machine(0) };
  const t = (sec: number, k: number): Tick => ({ ...tick(s(sec), running, 20, 11, 45, 1, k), ...machine(k) });
  return {
    name: 'machine-headroom', anchor: false,
    card: 'The headroom is 45 % of the machine, yet the machine is nearly full. Find out whether slot is holding builds back and what uses the rest.',
    state: st, history: history(running, 20, 11, 45, 1).map((h, k) => ({ ...h, ...machine(k) })),
    steps: [{ moment: 'loaded', dt_s: 1, events: [t(1, 1)] }, { moment: 'next tick', dt_s: 1, events: [t(2, 2)] }],
    output: {}, task: { kind: 'see', expect: 'over target' },
  };
}

export const SCENARIOS: Scenario[] = [
  busyFailed(), videoCall('video-call', false), videoCall('video-call-old-daemon', true), queuePosition(), crowded(), callMode(), upgradePending(), idle(),
  machineHeadroom(),
];

/** Scenarios whose output pane is open from the start. */
export const OPEN_PANE: Record<string, number> = { crowded: 100 };
