import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@/stores/toasts", () => ({ toast: vi.fn() }));
vi.stubGlobal("document", { hasFocus: () => true });

vi.stubGlobal("localStorage", { getItem: () => null, setItem: vi.fn() });
const { duration, leftNow, refreshClaude, runsOutAt, startUsageTracking, tightest, useUsage } = await import("./usage");

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
