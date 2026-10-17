/**
 * Picks the overall winner of a race or A/B test from its numbers: each agent scores 0–1 per
 * metric against the best one (best / value, lower is better), weighted and averaged over the
 * metrics known for every contender. Agents that errored, or did nothing while others changed
 * code, are out.
 */

export interface Contender {
  /** Time spent working (or run time), ms. */
  timeMs: number | null;
  cost: number | null;
  tokens: number | null;
  /** Lines and files changed; null while unknown. */
  changed: number | null;
  errored: boolean;
  finished: boolean;
}

export const WEIGHTS = { cost: 0.4, timeMs: 0.4, tokens: 0.2 } as const;
type Key = keyof typeof WEIGHTS;
const LABELS: Record<Key, string> = { cost: "cost", timeMs: "time", tokens: "tokens" };

/** Scores this close count as a tie. */
const TIE = 0.02;

export interface Verdict {
  /** Index of the winner, or null when there is none (tie, too few numbers, everyone out). */
  winner: number | null;
  /** Overall score per contender, 0–1; null when it's out or unscored. */
  scores: (number | null)[];
  /** Why each contender that's out is out. */
  out: (string | null)[];
  /** Whether every contender has finished, so the verdict is final. */
  final: boolean;
  /** One line explaining the verdict. */
  reason: string;
}

export function judge(cs: Contender[]): Verdict {
  const anyChanged = cs.some((c) => (c.changed ?? 0) > 0);
  const out = cs.map((c) => (c.errored ? "errored" : anyChanged && c.changed === 0 ? "changed nothing" : null));
  const final = cs.every((c) => c.finished);
  const scores: (number | null)[] = cs.map(() => null);
  const live = cs.map((_, i) => i).filter((i) => !out[i]);
  const none = (reason: string): Verdict => ({ winner: null, scores, out, final, reason });

  if (live.length === 0) return none("No agent finished the task cleanly.");
  if (live.length === 1) {
    scores[live[0]] = 1;
    return { winner: live[0], scores, out, final, reason: `The only agent that ${anyChanged ? "changed code" : "didn't error"}.` };
  }

  const keys = (Object.keys(WEIGHTS) as Key[]).filter((k) => live.every((i) => cs[i][k] !== null));
  if (!keys.length) return none("Not enough numbers yet to pick a winner.");
  const total = keys.reduce((n, k) => n + WEIGHTS[k], 0);
  const best = Object.fromEntries(keys.map((k) => [k, Math.min(...live.map((i) => cs[i][k]!))])) as Record<Key, number>;
  const part = (i: number, k: Key) => {
    const v = cs[i][k]!;
    return v <= 0 ? 1 : best[k] / v;
  };
  for (const i of live) scores[i] = keys.reduce((n, k) => n + WEIGHTS[k] * part(i, k), 0) / total;

  const ranked = [...live].sort((x, y) => scores[y]! - scores[x]!);
  const [first, second] = ranked;
  if (scores[first]! - scores[second]! < TIE) return none("Too close to call: the top agents score within 2%.");
  const wins = keys.filter((k) => cs[first][k] === best[k] && live.some((i) => i !== first && cs[i][k]! > best[k]));
  const reason = wins.length
    ? `Best overall; ${wins.length === keys.length ? "best" : "lowest"} on ${wins.map((k) => LABELS[k]).join(", ")}.`
    : "Best balance of cost, time and tokens.";
  return { winner: first, scores, out, final, reason };
}
