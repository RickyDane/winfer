import { beforeAll, describe, expect, it } from "vitest";

// The screens' modules read the page's location and listen to the window when imported.
let parseRoute: typeof import("../hooks").parseRoute;
let files: typeof import("./Files");
let kit: typeof import("./panelKit");
beforeAll(async () => {
  Object.assign(globalThis, { location: { hash: "" }, window: { addEventListener() {} } });
  ({ parseRoute } = await import("../hooks"));
  files = await import("./Files");
  kit = await import("./panelKit");
});

describe("side panel", () => {
  it("routes each module", () => {
    expect(parseRoute("#/panel")).toEqual({ name: "panel" });
    expect(parseRoute("#/panel/android")).toEqual({ name: "android", workspaceId: null });
    expect(parseRoute("#/panel/android/B~ws-1")).toEqual({ name: "android", workspaceId: "B~ws-1" });
    expect(parseRoute("#/panel/browser/w%201")).toEqual({ name: "browser", workspaceId: "w 1" });
    expect(parseRoute("#/panel/files/w1")).toEqual({ name: "files", workspaceId: "w1" });
    expect(parseRoute("#/panel/terminal/w1/side-terminal-w1")).toEqual({ name: "terminal", workspaceId: "w1", paneId: "side-terminal-w1" });
  });

  it("sends Ctrl+key as control characters", () => {
    const { withCtrl } = kit;
    expect(withCtrl("c")).toBe("\x03");
    expect(withCtrl("D")).toBe("\x04");
    expect(withCtrl("[")).toBe("\x1b");
    expect(withCtrl("1")).toBe("1");
  });

  it("round-trips terminal bytes as base64", () => {
    const { fromB64, textToB64 } = kit;
    const text = "héllo ✓\r\n";
    expect(new TextDecoder().decode(fromB64(textToB64(text)))).toBe(text);
  });

  it("shows review comments as quotes and paths from the workspace", () => {
    const { relative, showComments } = files;
    expect(showComments("a\n<!-- @user: fix\nthis -->\nb")).toBe("a\n\n> 💬 **user:** fix this\n\nb");
    expect(relative("/r/ws/src/a.ts", "/r/ws/")).toBe("src/a.ts");
    expect(relative("/elsewhere/a.ts", "/r/ws")).toBe("/elsewhere/a.ts");
  });
});
