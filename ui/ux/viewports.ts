export interface Viewport {
  width: number;
  height: number;
  gating: boolean;
}

/** Every merge holds the gates at the gating sizes; a round (UX_ALL=1) also measures the others. */
export const VIEWPORTS: Viewport[] = [
  { width: 1366, height: 768, gating: true },
  { width: 1280, height: 720, gating: true },
  { width: 1920, height: 1080, gating: false },
  { width: 1536, height: 754, gating: false },
  { width: 960, height: 1080, gating: false },
];

export const vpName = (v: Viewport): string => `${v.width}x${v.height}`;
