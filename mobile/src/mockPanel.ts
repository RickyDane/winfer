// `?mock=1`: the side panel of the in-memory desktop — a browser that draws its tabs, an Android
// emulator, a terminal that echoes and a few files — so the panel screens run without a desktop.
import type { DesktopMsg, PhoneMsg } from "./protocol";

type Emit = (msg: DesktopMsg) => void;

const b64 = (text: string) => btoa(unescape(encodeURIComponent(text)));

const FILES: Record<string, string> = {
  "/Users/me/code/winfer/README.md": "# winfer\n\nA workspace of terminal panes with coding agents.\n\n<!-- @user: mention the phone panel here -->\n\n## Build\n\n```sh\npnpm build\n```\n",
  "/Users/me/code/winfer/src/main.ts": 'import { start } from "./app";\n\nstart({ port: 5173 });\n',
  "/Users/me/code/winfer/package.json": '{\n  "name": "winfer",\n  "private": true\n}\n',
};
const ROOT = "/Users/me/code/winfer";

export class MockPanel {
  private tabs = [
    { id: "t1", url: "http://localhost:5173/", title: "winfer — dev", favicon: "" },
    { id: "t2", url: "https://example.com/", title: "Example Domain", favicon: "" },
  ];
  private active = "t1";
  private feed: { kind: "browser" | "android"; tab?: string } | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private seq = 0;
  private dots: { x: number; y: number }[] = [];
  private scrollY = 0;
  private typed = "";
  private files = { ...FILES };
  private docs = [`${ROOT}/README.md`];
  private line = "";
  private avdStarted = false;

  constructor(private emit: Emit) {}

  private frame() {
    if (!this.feed) return;
    const seq = ++this.seq;
    if (this.feed.kind === "browser") {
      const tab = this.tabs.find((t) => t.id === this.feed?.tab) ?? this.tabs[0];
      const [w, h] = [1024, 1400];
      const rows = Array.from({ length: 30 }, (_, i) => `<rect x="60" y="${260 + i * 90 - this.scrollY}" width="${900 - (i % 4) * 120}" height="22" rx="6" fill="#d4d4e0"/>`).join("");
      const dots = this.dots.map((d) => `<circle cx="${d.x}" cy="${d.y}" r="18" fill="#7c8cff" opacity="0.7"/>`).join("");
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="100%" height="100%" fill="#fafafa"/><rect width="100%" height="120" fill="#7c8cff"/><text x="60" y="78" font-size="44" font-family="sans-serif" fill="#fff">${tab.title}</text><text x="60" y="190" font-size="28" font-family="monospace" fill="#555">${tab.url} · typed: ${this.typed.replace(/[<&]/g, "")}</text>${rows}${dots}<text x="60" y="${h - 40}" font-size="24" fill="#999">frame ${seq}</text></svg>`;
      this.emit({ t: "panel.frame", feed: "browser", seq, data: b64(svg), width: w, height: h, mime: "image/svg+xml", workspaceId: "ws-winfer", tab: tab.id });
    } else {
      const [w, h] = [1080, 2400];
      const dots = this.dots.map((d) => `<circle cx="${d.x}" cy="${d.y}" r="40" fill="#fff" opacity="0.6"/>`).join("");
      const icons = Array.from({ length: 12 }, (_, i) => `<rect x="${110 + (i % 4) * 230}" y="${700 + Math.floor(i / 4) * 280}" width="160" height="160" rx="40" fill="hsl(${i * 30} 70% 60%)"/>`).join("");
      const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1e3a8a"/><stop offset="1" stop-color="#4c1d95"/></linearGradient></defs><rect width="100%" height="100%" fill="url(#g)"/><text x="540" y="420" text-anchor="middle" font-size="160" font-family="sans-serif" fill="#fff">12:${String(seq % 60).padStart(2, "0")}</text>${icons}${dots}<text x="540" y="2250" text-anchor="middle" font-size="40" fill="#fff9">${this.typed.replace(/[<&]/g, "") || "Pixel 8 · API 35"}</text></svg>`;
      this.emit({ t: "panel.frame", feed: "android", seq, data: b64(svg), width: w, height: h, mime: "image/svg+xml" });
    }
  }

  private watch(kind: "browser" | "android", tab?: string) {
    this.feed = { kind, tab };
    this.dots = [];
    if (!this.timer) this.timer = setInterval(() => this.frame(), 700);
    setTimeout(() => this.frame(), 50);
  }

  private unwatch() {
    this.feed = null;
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  private browserState() {
    return { workspaceId: "ws-winfer", ready: true, needsInstall: false, installing: false, error: null, tabs: this.tabs, active: this.active };
  }

  /** The reply's `value`, or undefined when it isn't a panel request. */
  handle(msg: PhoneMsg): { value?: unknown; error?: string } | undefined {
    const m = msg as Record<string, unknown> & PhoneMsg;
    switch (msg.t) {
      case "panel.frameDone":
        return {};
      case "panel.get":
        return {
          value: {
            workspaceId: msg.workspaceId,
            name: "winfer",
            root: ROOT,
            terminals: [
              { id: `side-terminal-${msg.workspaceId}`, title: "Terminal", provider: "shell", side: true, running: true },
              { id: "p-shell", title: "dev server", provider: "shell", side: false, running: true },
            ],
          },
        };
      case "term.attach":
        setTimeout(() => this.emit({ t: "term.snapshot", paneId: msg.paneId, data: b64(`\x1b[1;34mmock\x1b[0m terminal (${msg.cols}×${msg.rows})\r\n$ `) }), 80);
        return {};
      case "term.in": {
        const data = decodeURIComponent(escape(atob(msg.data)));
        let out = "";
        for (const ch of data) {
          if (ch === "\r") {
            out += `\r\n${this.line ? `ran: ${this.line}\r\n` : ""}$ `;
            this.line = "";
          } else if (ch === "\x7f") {
            if (this.line) out += "\b \b";
            this.line = this.line.slice(0, -1);
          } else if (ch === "\x03") {
            out += "^C\r\n$ ";
            this.line = "";
          } else if (ch >= " ") {
            this.line += ch;
            out += ch;
          }
        }
        if (out) this.emit({ t: "term.out", paneId: msg.paneId, data: b64(out) });
        return {};
      }
      case "term.resize":
      case "term.detach":
        return {};
      case "browser.call": {
        const tab = m.tab as string;
        switch (msg.op) {
          case "state":
            return { value: this.browserState() };
          case "watch":
          case "select":
            this.active = tab;
            if (msg.op === "watch") this.watch("browser", tab);
            return {};
          case "unwatch":
            this.unwatch();
            return {};
          case "input": {
            const p = m.params as { type?: string; x?: number; y?: number; deltaY?: number; text?: string; key?: string };
            if (p.type === "mousePressed") this.dots.push({ x: p.x ?? 0, y: p.y ?? 0 });
            if (p.type === "mouseWheel") this.scrollY = Math.max(0, this.scrollY + (p.deltaY ?? 0));
            if (m.method === "Input.insertText") this.typed += p.text ?? "";
            if (p.type === "rawKeyDown" && p.key === "Backspace") this.typed = this.typed.slice(0, -1);
            return {};
          }
          case "newTab": {
            const id = `t${Date.now()}`;
            const url = (m.url as string) || "about:blank";
            this.tabs.push({ id, url, title: url === "about:blank" ? "" : url, favicon: "" });
            this.active = id;
            return { value: id };
          }
          case "closeTab":
            this.tabs = this.tabs.filter((t) => t.id !== tab);
            return {};
          case "navigate": {
            const t = this.tabs.find((x) => x.id === tab);
            if (t) Object.assign(t, { url: m.url as string, title: m.url as string });
            return {};
          }
          default:
            return {};
        }
      }
      case "android.call":
        switch (msg.op) {
          case "state":
          case "watch":
            if (msg.op === "watch") this.watch("android");
            return {
              value: {
                loaded: true,
                adb: true,
                devices: this.avdStarted ? [{ serial: "emulator-5554", state: "device", model: "Pixel 8", emulator: true }] : [],
                avds: ["Pixel_8_API_35"],
                current: this.avdStarted ? "emulator-5554" : null,
                starting: null,
                error: null,
                mirror: this.avdStarted ? { serial: "emulator-5554", mode: "scrcpy", error: null, showing: true, live: true } : null,
              },
            };
          case "startAvd":
            this.avdStarted = true;
            return {};
          case "unwatch":
            this.unwatch();
            return {};
          case "touch":
            if (m.kind === "down") this.dots.push({ x: (m.x as number) * 1080, y: (m.y as number) * 2400 });
            return {};
          case "text":
            this.typed += m.text as string;
            return {};
          default:
            return {};
        }
      case "files.call": {
        const path = m.path as string;
        switch (msg.op) {
          case "docs":
            return { value: { root: ROOT, docs: this.docs, active: this.docs[0] ?? null } };
          case "list": {
            const dir = ((m.dir as string) || ROOT).replace(/\/+$/, "");
            const names = new Map<string, { dir: boolean; size: number }>();
            for (const [p, text] of Object.entries(this.files)) {
              if (!p.startsWith(dir + "/")) continue;
              const [first, ...rest] = p.slice(dir.length + 1).split("/");
              names.set(first, { dir: rest.length > 0, size: rest.length ? 0 : text.length });
            }
            const entries = [...names].map(([name, e]) => ({ name, ...e })).sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name));
            return { value: { root: ROOT, dir, entries } };
          }
          case "read": {
            const text = this.files[path];
            if (text === undefined) return { error: "can't open it" };
            return { value: { path, kind: path.endsWith(".md") ? "markdown" : "text", text, size: text.length, modified: 1 } };
          }
          case "write":
            this.files[path] = m.text as string;
            return { value: { modified: 2 } };
          case "comment": {
            const lines = this.files[path].split("\n");
            lines.splice(m.afterLine as number, 0, "", `<!-- @user: ${m.body as string} -->`, "");
            this.files[path] = lines.join("\n");
            return { value: { modified: 3 } };
          }
          case "open":
            if (!this.docs.includes(path)) this.docs.push(path);
            return {};
          case "close":
            this.docs = this.docs.filter((d) => d !== path);
            return {};
        }
      }
    }
    return undefined;
  }
}
