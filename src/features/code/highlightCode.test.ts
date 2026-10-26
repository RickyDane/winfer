import { describe, expect, it, vi } from "vitest";
import { createCodeHighlighter, type HighlightRequest } from "./highlightCode";

function setup() {
  const worker = { postMessage: vi.fn(), terminate: vi.fn(), onmessage: null, onerror: null, onmessageerror: null };
  const make = vi.fn(() => worker as unknown as Worker);
  const highlight = createCodeHighlighter(make);
  const reply = (id: number, html: string | null) =>
    (worker as unknown as Worker).onmessage!({ data: { id, html } } as MessageEvent);
  return { worker, make, highlight, reply };
}

describe("off-thread code highlighting", () => {
  it("starts lazily and skips plain or oversized code without starting a worker", async () => {
    const { make, highlight } = setup();
    expect(make).not.toHaveBeenCalled();
    for (const [code, lang] of [["abc", "text"], ["abc", ""], ["x".repeat(50_001), "ts"], ["x\n".repeat(1001), "ts"]])
      expect(await highlight(code, lang, true)).toBeNull();
    expect(make).not.toHaveBeenCalled();
  });

  it("deduplicates pending and completed snippets across panes", async () => {
    const { make, worker, highlight, reply } = setup();
    const first = highlight("const x = 1", "ts", true);
    expect(highlight("const x = 1", "ts", true)).toBe(first);
    expect(worker.postMessage).toHaveBeenCalledTimes(1);
    const request = worker.postMessage.mock.calls[0][0] as HighlightRequest;
    reply(request.id, "<pre>highlighted</pre>");
    expect(await first).toBe("<pre>highlighted</pre>");
    expect(highlight("const x = 1", "ts", true)).toBe(first);
    const light = highlight("const x = 1", "ts", false);
    reply(2, "<pre>light</pre>");
    expect(await light).toBe("<pre>light</pre>");
    expect(make).toHaveBeenCalledTimes(1);
  });

  it("bounds the snippet cache", () => {
    const { worker, highlight, reply } = setup();
    for (let i = 0; i < 65; i++) {
      void highlight(`const n = ${i}`, "ts", true);
      reply(i + 1, `<pre>${i}</pre>`);
    }
    void highlight("const n = 0", "ts", true);
    expect(worker.postMessage).toHaveBeenCalledTimes(66);
    reply(66, "<pre>0</pre>");
  });

  it("bounds retained HTML as well as source text", async () => {
    const { worker, highlight, reply } = setup();
    const first = highlight("small input", "ts", true);
    reply(1, "x".repeat(2_000_001));
    await first;
    void highlight("small input", "ts", true);
    expect(worker.postMessage).toHaveBeenCalledTimes(2);
    reply(2, null);
  });

  it("resolves outstanding work as plain text on worker failure", async () => {
    const { worker, make, highlight } = setup();
    const first = highlight("one", "ts", true);
    const second = highlight("two", "ts", true);
    (worker as unknown as Worker).onerror!(new Event("error") as ErrorEvent);
    expect(await first).toBeNull();
    expect(await second).toBeNull();
    expect(await highlight("three", "ts", true)).toBeNull();
    expect(worker.terminate).toHaveBeenCalledOnce();
    expect(make).toHaveBeenCalledOnce();
  });

  it("falls back when workers are unavailable (including CSP restrictions)", async () => {
    const highlight = createCodeHighlighter(() => { throw new Error("Blocked"); });
    expect(await highlight("one", "ts", true)).toBeNull();
  });
});
