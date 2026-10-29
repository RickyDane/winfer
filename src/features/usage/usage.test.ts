import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@/stores/toasts", () => ({ toast: vi.fn() }));
vi.stubGlobal("document", { hasFocus: () => true });

vi.stubGlobal("localStorage", { getItem: () => null, setItem: vi.fn() });
const { duration, leftNow, pooled, pooledRunsOutAt, pooledSamples, refreshClaude, runsOutAt, startUsageTracking, tightest, useUsage } = await import("./usage");

const HOUR = 3600_000;
const now = 1_800_000_000_000;
const session = { kind: "session", label: "5-hour", usedPercent: 40, resetsAt: now + 3 * HOUR };

describe("usage", () => {
  it("counts a window whose reset has passed as full", () => {
    expect(leftNow(session, now)).toBe(60);
    expect(leftNow({ ...session, resetsAt: now - 1 }, now)).toBe(100);
  });

  it("shows the window with the least left", () => {
    const week = { kind: "week", label: "Weekly", usedPercent: 90, resetsAt: now + 50 * HOUR };
    expect(tightest({ provider: "claude", at: now, windows: [session, week] }, now)?.kind).toBe("week");
  });

  it("counts monthly credits only once the plan limits are out", () => {
    const credit = { kind: "month-credit", label: "Monthly credit", usedPercent: 100, resetsAt: now + 500 * HOUR };
    expect(tightest({ provider: "codex", at: now, windows: [session, credit] }, now)?.kind).toBe("session");
    const out = { ...session, usedPercent: 100 };
    expect(tightest({ provider: "codex", at: now, windows: [out, { ...credit, usedPercent: 20 }] }, now)?.kind).toBe("month-credit");
  });

  it("projects running out only when that's before the reset", () => {
    const key = `claude:session:${session.resetsAt}`;
    // 20% in the last hour: the remaining 60% takes 3h, just past the reset.
    expect(runsOutAt("claude", session, { [key]: [[now - HOUR, 20], [now, 40]] }, now)).toBeNull();
    // 30% in the last hour: out in 2h, before the reset in 3h.
    expect(runsOutAt("claude", session, { [key]: [[now - HOUR, 10], [now, 40]] }, now)).toBe(now + 2 * HOUR);
    // Too short a history to say.
    expect(runsOutAt("claude", session, { [key]: [[now - 60_000, 30], [now, 40]] }, now)).toBeNull();
  });

  it("formats durations", () => {
    expect(duration(108 * 60_000)).toBe("1h 48m");
    expect(duration(3 * 86400_000 + 4 * HOUR)).toBe("3d 4h");
    expect(duration(20_000)).toBe("1m");
  });
});

describe("Claude usage refresh", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    useUsage.setState({ snapshots: {}, samples: {}, refreshing: {}, errors: {} });
  });

  it("replaces a stale reading with the exhausted live session", async () => {
    useUsage.setState({ snapshots: { claude: { provider: "claude", at: 1, windows: [{ ...session, usedPercent: 1 }] } } });
    const live = { provider: "claude", at: Date.now(), windows: [{ ...session, usedPercent: 100 }] };
    vi.mocked(invoke).mockResolvedValue(live);
    await refreshClaude();
    expect(invoke).toHaveBeenCalledWith("usage_claude");
    expect(useUsage.getState().snapshots.claude).toEqual(live);
    expect(leftNow(useUsage.getState().snapshots.claude.windows[0], now)).toBe(0);
    expect(useUsage.getState().refreshing.claude).toBe(false);
  });

  it("keeps the reading and timestamp when refresh fails, then clears the error on retry", async () => {
    const saved = { provider: "claude", at: 1, windows: [session] };
    useUsage.setState({ snapshots: { claude: saved } });
    vi.mocked(invoke).mockRejectedValueOnce("Sign-in expired");
    await refreshClaude();
    expect(useUsage.getState().snapshots.claude).toEqual(saved);
    expect(useUsage.getState().errors.claude).toBe("Sign-in expired");
    expect(useUsage.getState().refreshing.claude).toBe(false);
    vi.mocked(invoke).mockResolvedValue({ ...saved, at: 2 });
    await refreshClaude();
    expect(useUsage.getState().errors.claude).toBeUndefined();
  });

  it("shares concurrent manual and automatic requests", async () => {
    let resolve!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(() => new Promise((done) => { resolve = done; }));
    const first = refreshClaude();
    const second = refreshClaude();
    expect(first).toBe(second);
    expect(useUsage.getState().refreshing.claude).toBe(true);
    await Promise.resolve();
    resolve({ provider: "claude", at: 2, windows: [session] });
    await first;
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(useUsage.getState().refreshing.claude).toBe(false);
  });

  it("polls Claude at startup and after five minutes even when status reports arrive", async () => {
    const timers = { setTimeout: vi.fn(), setInterval: vi.fn(), addEventListener: vi.fn() };
    vi.stubGlobal("window", timers);
    vi.spyOn(Date, "now").mockReturnValue(now);
    const saved = { provider: "claude", at: now, windows: [session] };
    useUsage.setState({ snapshots: { claude: saved } });
    vi.mocked(invoke).mockImplementation(async (command) => command === "usage_claude" ? saved : null);
    startUsageTracking();
    await refreshClaude(); // Joins the startup request.
    expect(vi.mocked(invoke).mock.calls.filter(([c]) => c === "usage_claude")).toHaveLength(1);
    const report = vi.mocked(listen).mock.calls.find(([event]) => event === "usage://report")![1];
    report({ payload: { provider: "claude", paneId: "p", report: { windows: [{ ...session, usedPercent: 1 }] } } } as never);
    expect(useUsage.getState().snapshots.claude).toEqual(saved); // Cached status line cannot overwrite live limits.
    const poll = timers.setInterval.mock.calls.find(([, ms]) => ms === 60_000)![0];
    poll();
    await Promise.resolve();
    expect(vi.mocked(invoke).mock.calls.filter(([c]) => c === "usage_claude")).toHaveLength(1);
    vi.mocked(Date.now).mockReturnValue(now + 5 * 60_000);
    poll();
    await refreshClaude();
    expect(vi.mocked(invoke).mock.calls.filter(([c]) => c === "usage_claude")).toHaveLength(2);
    vi.mocked(Date.now).mockRestore();
  });
});

describe("pooled accounts", () => {
  const account = (id: string, enabled = true, signedIn = true) => ({ id, provider: "claude", label: id, enabled, dir: "", signedIn, exhaustedUntil: null });
  const accounts = [account("claude:default"), account("claude:account-2"), account("claude:account-3", false)];
  const snap = (provider: string, used: number, resetsAt: number, at = now) => ({ provider, at, windows: [{ kind: "session", label: "5-hour", usedPercent: used, resetsAt }] });
  const snapshots = {
    claude: snap("claude", 100, now + 1 * HOUR),
    "claude:account-2": snap("claude:account-2", 22, now + 4 * HOUR, now - HOUR),
    "claude:account-3": snap("claude:account-3", 0, now + 2 * HOUR),
  };

  it("averages what the counted accounts have left, resetting with the first", () => {
    const p = pooled("claude", snapshots, accounts, now)!;
    expect(leftNow(p.windows[0], now)).toBe(39);
    expect(p.windows[0].resetsAt).toBe(now + HOUR);
    expect(p.at).toBe(now - HOUR);
  });

  it("is the default reading with a single account", () => {
    expect(pooled("claude", snapshots, [], now)).toEqual(snapshots.claude);
  });

  it("averages the readings over time and projects the combined pace", () => {
    const samples = {
      [`claude:session:${now + HOUR}`]: [[now - HOUR, 80], [now, 100]] as [number, number][],
      [`claude:account-2:session:${now + 4 * HOUR}`]: [[now - 30 * 60_000, 2], [now, 22]] as [number, number][],
    };
    expect(pooledSamples("claude", "session", snapshots, accounts, samples, now)).toEqual([[now - HOUR, 41], [now - 30 * 60_000, 41], [now, 61]]);
    // 78% left in total, used at 20%/h + 40%/h: out in 1.3h, after the default's reset in 1h.
    expect(pooledRunsOutAt("claude", "session", snapshots, accounts, samples, now)).toBeNull();
    const later = { ...snapshots, claude: snap("claude", 100, now + 3 * HOUR) };
    const laterSamples = { ...samples, [`claude:session:${now + 3 * HOUR}`]: samples[`claude:session:${now + HOUR}`] };
    expect(pooledRunsOutAt("claude", "session", later, accounts, laterSamples, now)).toBe(now + 1.3 * HOUR);
  });
});
