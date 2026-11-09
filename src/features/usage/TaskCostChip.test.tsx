import { beforeEach, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import type { PaneSpend } from "./usage";
import type { Pane } from "@/stores/workspaces";

const state = vi.hoisted(() => ({ show: true, spend: undefined as PaneSpend | undefined }));
vi.mock("@/stores/workspaces", () => ({
  useWorkspaces: (select: (s: unknown) => unknown) => select({ showPaneCost: state.show }),
}));
vi.mock("./usage", () => ({
  useUsage: (select: (s: unknown) => unknown) => select({ paneSpend: { pane: state.spend } }),
}));
vi.mock("./taskCost", () => ({
  useTaskCost: () => undefined,
  useSessionCost: () => undefined,
}));

import { PaneCost } from "./TaskCostChip";

const pane = { id: "pane", provider: "winfer" } as Pane;
beforeEach(() => {
  state.show = true;
  state.spend = { cost: 1.25, input: 1000, cached: 500, output: 100, at: 42 };
});

it("shows reported session costs for either winfer code UI", () => {
  const html = renderToStaticMarkup(<PaneCost pane={pane} />);
  expect(html).toContain("$1.25");
  expect(html).not.toContain("≈");
});

it("labels token-based estimates with their pricing model", () => {
  state.spend = { ...state.spend!, estimated: true, model: "gpt-5.4" };
  const html = renderToStaticMarkup(<PaneCost pane={pane} />);
  expect(html).toContain("≈$1.25");
  expect(html).toContain("gpt-5.4");
  expect(html).toContain("other models and service tiers may differ");
});

it("keeps unknown prices unknown and honors the pane cost setting", () => {
  state.spend = { ...state.spend!, cost: null };
  expect(renderToStaticMarkup(<PaneCost pane={pane} />)).toContain("No price reported");
  state.show = false;
  expect(renderToStaticMarkup(<PaneCost pane={pane} />)).toBe("");
});

it("shows a reported cost even when no token counts were supplied", () => {
  state.spend = { ...state.spend!, input: 0, output: 0 };
  expect(renderToStaticMarkup(<PaneCost pane={pane} />)).toContain("$1.25");
});
