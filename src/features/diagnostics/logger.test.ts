import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async (..._args: unknown[]) => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

vi.stubGlobal("window", {
  __TAURI_INTERNALS__: {},
  setTimeout: (fn: () => void, ms?: number) => setTimeout(fn, ms) as unknown as number,
  clearTimeout: (id: number) => clearTimeout(id as unknown as ReturnType<typeof setTimeout>),
  addEventListener: () => {},
});
vi.stubGlobal("document", { addEventListener: () => {}, visibilityState: "visible" });
vi.stubGlobal("navigator", { userAgent: "test" });

const { logger, flushLog, pendingEntries } = await import("./logger");

type SentEntries = { entries: { level: string; message: string }[] };
const sent = (call: number): SentEntries => invoke.mock.calls[call][1] as SentEntries;

describe("frontend logger", () => {
  beforeEach(async () => {
    await flushLog();
    invoke.mockClear();
  });

  it("batches info entries and flushes errors immediately", async () => {
    logger.info("test", "hello", 42);
    expect(pendingEntries()).toHaveLength(1);
    expect(invoke).not.toHaveBeenCalled();
    logger.error("test", new Error("boom"));
    // Error flush is fire-and-forget; let it run.
    await vi.waitFor(() => expect(invoke).toHaveBeenCalled());
    const entries = sent(0).entries;
    expect(entries).toHaveLength(2);
    expect(entries[0].message).toContain("hello 42");
    expect(entries[1].message).toContain("boom");
    expect(pendingEntries()).toHaveLength(0);
  });

  it("caps queued messages at 4096 chars", async () => {
    logger.info("test", "x".repeat(10_000));
    await flushLog();
    const entries = sent(invoke.mock.calls.length - 1).entries;
    expect(entries[0].message.length).toBeLessThanOrEqual(4096);
  });
});
