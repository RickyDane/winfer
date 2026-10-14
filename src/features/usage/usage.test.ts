import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), sendNotification: vi.fn() }));

const { duration, leftNow, runsOutAt, tightest } = await import("./usage");

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
