import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@/stores/toasts", () => ({ toast: vi.fn() }));
vi.stubGlobal("document", { hasFocus: () => true });
vi.stubGlobal("localStorage", { getItem: () => null, setItem: vi.fn() });

import type { PoolMember } from "./usage";

const { AccountBreakdown } = await import("./UsageMeter");

const now = Date.now();
const windows = [{ kind: "session", label: "5-hour", usedPercent: 40, resetsAt: now + 3 * 3600_000 }];
const member = (id: string, label: string, email: string | null = null): PoolMember => ({
  id, key: id === "claude:default" ? "claude" : id, label, email,
  snapshot: { provider: "claude", at: now, windows }, counted: true,
});
const members = [member("claude:default", "Default", "default@example.com"), member("claude:account-2", "Account 2", "second@example.com"), member("claude:account-3", "Account 3")];
const render = (startCollapsed?: boolean) =>
  renderToStaticMarkup(createElement(AccountBreakdown, { members, samples: {}, provider: "claude", providerLabel: "Claude Code", startCollapsed }));

describe("AccountBreakdown", () => {
  it("hides the individual accounts by default when started collapsed (the popup)", () => {
    const html = render(true);
    expect(html).toContain("Per account");
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain("Account 2");
    expect(html).not.toContain("Account 3");
  });

  it("shows the individual accounts by default otherwise (Settings)", () => {
    const html = render();
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain("Account 2");
    expect(html).toContain("Account 3");
  });

  it("shows the exact reset time of each account's 5-hour and weekly windows", () => {
    const two = [
      { kind: "session", label: "5-hour", usedPercent: 40, resetsAt: now + 3 * 3600_000 },
      { kind: "week", label: "Weekly", usedPercent: 90, resetsAt: now + 50 * 3600_000 },
    ];
    const one: PoolMember[] = [{ id: "claude:default", key: "claude", label: "Default", email: null, snapshot: { provider: "claude", at: now, windows: two }, counted: true }];
    const html = renderToStaticMarkup(createElement(AccountBreakdown, { members: one, samples: {}, provider: "claude", providerLabel: "Claude Code" }));
    expect(html).toContain("5-hour");
    expect(html).toContain("Weekly");
    expect(html).toContain("resets ");
    expect(html).toContain("(in 3h)");
    expect(html).toContain("(in 2d 2h)");
  });

  it("keeps the exact reset visible when a run-out is projected", () => {
    const one: PoolMember[] = [{ id: "claude:default", key: "claude", label: "Default", email: null, snapshot: { provider: "claude", at: now, windows }, counted: true }];
    const samples = { [`claude:session:${windows[0].resetsAt}`]: [[now - 3600_000, 10], [now, 40]] as [number, number][] };
    const html = renderToStaticMarkup(createElement(AccountBreakdown, { members: one, samples, provider: "claude", providerLabel: "Claude Code" }));
    expect(html).toContain("resets ");
    expect(html).toContain("(in 3h)");
    expect(html).toContain("at this pace runs out");
    expect(html).toContain("(in 2h)");
  });

  it("shows each account's email when known, and nothing when not", () => {
    const html = render();
    expect(html).toContain("default@example.com");
    expect(html).toContain("second@example.com");
    // Account 3 has no email on file: no empty placeholder is rendered for it.
    expect(html.match(/>[^<]*@example\.com</g)).toHaveLength(2);
  });
});
