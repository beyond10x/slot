export interface Viewport {
  width: number;
  height: number;
  gating: boolean;
}

/** Every merge holds the gates at the gating sizes; a round (UX_ALL=1) also measures the others. */
export const VIEWPORTS: Viewport[] = [
  { width: 1366, height: 768, gating: true },
  { width: 1280, height: 720, gating: true },
  { width: 1440, height: 900, gating: false },
  { width: 1280, height: 800, gating: false },
  { width: 1024, height: 768, gating: false },
  { width: 768, height: 1024, gating: false },
  { width: 390, height: 844, gating: false },
];

export const THEMES = ['light', 'dark'] as const;
export type Theme = (typeof THEMES)[number];

/** The size the ratchet compares; the other sizes are reported in a round. */
export const RATCHET_VIEWPORT = '1366x768';

/** Docusaurus moves the sidebar, search and theme toggle into a drawer below this width. */
export const DESKTOP_MIN_WIDTH = 997;

export const vpName = (v: Viewport): string => `${v.width}x${v.height}`;

export const viewportsInRun = (): Viewport[] => (process.env.UX_ALL ? VIEWPORTS : VIEWPORTS.filter((v) => v.gating));
