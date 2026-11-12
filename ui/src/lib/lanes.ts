export interface Placed<T> {
  item: T;
  lane: number;
}

/**
 * Greedy lane assignment: items in order of start, each into the lowest lane
 * whose last item ended at or before its start. `end` may be Infinity for
 * items still open.
 */
export function assignLanes<T>(
  items: readonly T[],
  start: (t: T) => number,
  end: (t: T) => number,
): { placed: Placed<T>[]; lanes: number } {
  const sorted = [...items].sort((a, b) => start(a) - start(b));
  const ends: number[] = [];
  const placed: Placed<T>[] = [];
  for (const item of sorted) {
    const s = start(item);
    let lane = ends.findIndex((e) => e <= s);
    if (lane < 0) {
      lane = ends.length;
      ends.push(end(item));
    } else {
      ends[lane] = end(item);
    }
    placed.push({ item, lane });
  }
  return { placed, lanes: ends.length };
}
