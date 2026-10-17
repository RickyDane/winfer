// Half-open time ranges [start, end) over Date objects. Day boundaries are UTC midnight.

const DAY = 24 * 60 * 60 * 1000;

/** "2024-01-01T10:00Z/2024-01-01T12:00Z" -> { start, end } */
export function parse(text) {
  const [a, b] = text.split("/");
  const start = new Date(a);
  const end = new Date(b);
  if (Number.isNaN(+start) || Number.isNaN(+end)) throw new RangeError(`bad range: ${text}`);
  if (end < start) throw new RangeError(`end before start: ${text}`);
  return { start, end };
}

export function format(r) {
  return `${r.start.toISOString()}/${r.end.toISOString()}`;
}

export function durationMs(r) {
  return r.end - r.start;
}

/** True if the ranges share at least one instant. Touching ranges don't overlap. */
export function overlaps(a, b) {
  return a.start <= b.end && b.start <= a.end;
}

/** Sorted, non-overlapping ranges covering the same instants; touching ranges are joined.
 *  Empty ranges are dropped. Doesn't modify its input. */
export function merge(ranges) {
  const sorted = ranges.filter((r) => r.end > r.start).sort((a, b) => a.start - b.start);
  const out = [];
  for (const r of sorted) {
    const last = out[out.length - 1];
    if (last && r.start <= last.end) {
      last.end = r.end;
    } else {
      out.push(r);
    }
  }
  return out;
}

/** The parts of `within` not covered by any of `ranges`. */
export function gaps(ranges, within) {
  const out = [];
  let cursor = within.start;
  for (const r of merge(ranges)) {
    if (r.end <= within.start || r.start >= within.end) continue;
    if (r.start > cursor) out.push({ start: cursor, end: r.start });
    if (r.end > cursor) cursor = r.end;
  }
  if (cursor < within.end) out.push({ start: cursor, end: within.end });
  return out;
}

/** Total time covered by the ranges, counting overlapping time once. */
export function totalMs(ranges) {
  return merge(ranges).reduce((sum, r) => sum + durationMs(r), 0);
}

/** Splits a range at each UTC midnight into per-day pieces. An empty range gives []. */
export function splitByDay(r) {
  const out = [];
  let start = r.start;
  while (start < r.end || out.length === 0) {
    const midnight = new Date(Math.floor(+start / DAY) * DAY + DAY);
    const end = midnight < r.end ? midnight : r.end;
    out.push({ start, end });
    start = end;
    if (+end === +r.end) break;
  }
  return out;
}
