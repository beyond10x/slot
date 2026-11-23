import type { Job, SharesView } from '../generated/types.ts';

/** The slots column of a job: by share what it uses of the share it asked for, by slots its slots. */
export function shareCell(j: Job): { text: string; title: string } {
  if (j.share == null) {
    if (j.state === 'queued' && j.collected) return { text: `${j.collected}/${j.tokens}`, title: `holds ${j.collected} of the ${j.tokens} slots it asked for` };
    return { text: String(j.tokens), title: '' };
  }
  if (j.state === 'queued') return { text: j.share.toFixed(1), title: `asks for a share of ${j.share} CPUs` };
  if (j.share_used == null) return { text: j.share.toFixed(1), title: `asked for a share of ${j.share} CPUs` };
  return {
    text: `${j.share_used.toFixed(1)}/${j.share.toFixed(1)}`,
    title: `uses ${j.share_used} of the ${j.share} CPUs it asked for`,
  };
}

/** The status line of admission by share; empty by slots or from a daemon that has none. */
export function sharesLine(v: SharesView | null | undefined): string {
  if (!v || v.admission !== 'shares') return '';
  const parts = [
    `by share: ${Math.max(0, v.free).toFixed(1)} of a budget of ${v.budget.toFixed(1)} CPUs free (cap ${v.cap.toFixed(1)})`,
    `${v.running} running (${v.floor} to ${v.max})`,
  ];
  if (v.idle > 0) parts.push(`${v.idle} idle lend ${v.lent.toFixed(1)}`);
  if (v.braking) parts.push(`CPU pressure ${v.pressure.toFixed(0)} % over ${v.pressure_max.toFixed(0)} %: only short jobs start`);
  return parts.join(' · ');
}
