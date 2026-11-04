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
const member = (id: string, label: string): PoolMember => ({
  id, key: id === "claude:default" ? "claude" : id, label,
  snapshot: { provider: "claude", at: now, windows }, counted: true,
});
const members = [member("claude:default", "Default"), member("claude:account-2", "Account 2"), member("claude:account-3", "Account 3")];
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
});
