import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const { formatCost, formatTokens, modelTotals, toBars } = await import("./history");

const HOUR = 3600_000;
const row = (hour: number, provider: string, model: string, cost: number) => ({
  hour,
  provider,
  model,
  input: 100,
  output: 10,
  cacheRead: 1000,
  cacheWrite: 0,
  cost,
  priced: true,
});

describe("usage history", () => {
  const now = new Date(2026, 9, 14, 15, 30).getTime();

  it("makes one bar per step, the last holding now", () => {
    const bars = toBars([], "24h", now);
    expect(bars).toHaveLength(24);
    expect(bars[23].start).toBe(new Date(2026, 9, 14, 15).getTime());
    expect(toBars([], "7d", now)).toHaveLength(28);
    const days = toBars([], "90d", now);
    expect(days).toHaveLength(90);
    expect(days[89].start).toBe(new Date(2026, 9, 14).getTime());
  });

  it("sums rows into their bars per provider and skips older ones", () => {
    const hour = new Date(2026, 9, 14, 14).getTime();
    const bars = toBars(
      [row(hour, "claude", "a", 1), row(hour, "claude", "b", 2), row(hour, "codex", "c", 0.5), row(hour - 30 * HOUR, "claude", "a", 9)],
      "24h",
      now,
    );
    expect(bars[22].cost).toEqual({ claude: 3, codex: 0.5 });
    expect(bars[22].tokens.claude).toBe(2220);
    expect(bars.reduce((s, b) => s + (b.cost.claude ?? 0), 0)).toBe(3);
  });

  it("totals models, most expensive first", () => {
    const t = modelTotals([row(10, "claude", "a", 1), row(20, "codex", "c", 5), row(30, "claude", "a", 1), row(0, "claude", "old", 99)], 5);
    expect(t.map((m) => [m.model, m.cost])).toEqual([
      ["c", 5],
      ["a", 2],
    ]);
  });

  it("formats", () => {
    expect(formatTokens(2_692_091_473)).toBe("2.69B");
    expect(formatTokens(15_400)).toBe("15K");
    expect(formatCost(0.004)).toBe("<$0.01");
    expect(formatCost(1061.55)).toBe("$1,062");
  });
});
