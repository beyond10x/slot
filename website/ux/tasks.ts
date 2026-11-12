// Scenario cards of the docs site: a reader's task in their own words, the pages that answer it,
// and the text on each page that is the answer. Two anchors run every round; the others rotate.

export interface Task {
  id: string;
  /** The reader's question. */
  question: string;
  /** Words the reader scans links for, lower case, most specific first. */
  words: string[];
  /** Answering pages: route relative to the site base, and the text that is the answer there. */
  answers: Array<[string, string]>;
  anchor: boolean;
}

export const TASKS: Task[] = [
  {
    id: 'install',
    question: 'How do I install slot?',
    words: ['install', 'get started', 'getting started'],
    answers: [['docs/getting-started', 'install -D -m 0755']],
    anchor: true,
  },
  {
    id: 'headroom',
    question: 'How do I keep CPU free for my video call?',
    words: ['headroom', 'call', 'cpu'],
    answers: [
      ['docs/headroom', 'slot headroom 30'],
      ['docs/headroom', 'While a call uses your microphone or your camera'],
    ],
    anchor: true,
  },
  {
    id: 'cancel',
    question: 'How do I cancel a job from another shell?',
    words: ['cancel', 'stop', 'cli'],
    answers: [
      ['docs/reference/cli', 'slot cancel [OPTIONS]'],
      ['docs/getting-started', 'slot cancel 3'],
      ['docs/how-it-works', 'slot cancel <id>'],
    ],
    anchor: false,
  },
  {
    id: 'remote-env',
    question: 'Which environment variables reach a remote build server?',
    words: ['environment', 'remote', 'variables'],
    answers: [
      ['docs/reference/environment', 'extra variable names forwarded to a remote host'],
      ['docs/remote-builds', 'export SLOT_ENV='],
    ],
    anchor: false,
  },
  {
    id: 'upgrade',
    question: 'How do I upgrade the running daemon?',
    words: ['upgrade', 'update', 'get started'],
    answers: [['docs/getting-started', 'systemctl --user reload slot']],
    anchor: false,
  },
  {
    id: 'disk',
    question: 'What does slot do when the disk is nearly full?',
    words: ['disk', 'how it works'],
    answers: [['docs/how-it-works', 'Waiting for disk and memory']],
    anchor: false,
  },
  {
    id: 'plugin',
    question: 'How do I add the Claude Code plugin?',
    words: ['plugin', 'claude'],
    answers: [['docs/claude-code-plugin', 'claude plugin install slot@slot']],
    anchor: false,
  },
];

/** Links a reader tries when no link names the task: the site's own way into the docs. */
export const FALLBACK_LINKS = ['documentation', 'overview'];
