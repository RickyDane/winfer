/**
 * Dev-only demo data for marketing screenshots: open the dev server with `?showcase=<scene>`
 * (hero, overview, review, usage, sessions, light, knowledge, bench) and the mock backend serves this state,
 * realistic agent sessions and a fake app in the browser. `scripts/screenshots.sh` captures
 * every scene into docs/screenshots.
 */

export const SHOWCASE = new URLSearchParams(location.search).get("showcase");

/** Scenes that show a plugin page instead of the workspace, keyed by scene. */
const PAGE_SCENES: Record<string, string> = {
  knowledge: "page:knowledge-base/galaxy",
  bench: "page:harness-bench/bench",
};
// Headless Chrome's virtual time (scripts/screenshots.sh) leaves CSS animations and transitions
// half-done in the capture, so the showcase shows every element in its final state.
if (SHOWCASE) {
  const style = document.createElement("style");
  style.textContent = "*, *::before, *::after { animation: none !important; transition: none !important; }";
  document.head.append(style);
}

/** Scenes without the browser panel. */
const NO_BROWSER = SHOWCASE === "overview" || SHOWCASE === "review" || (SHOWCASE ?? "") in PAGE_SCENES;

const ROOT = "/Users/maya/code";
const wt = (repo: string, name: string, branch = `winfer/${name}`) => ({
  repo: `${ROOT}/${repo}`,
  path: `${ROOT}/${repo}/.worktrees/${name}`,
  branch,
  base: "main",
});

type PaneSpec = {
  id: string;
  provider: string;
  title: string;
  repo: string;
  worktree: ReturnType<typeof wt> | null;
  agentTitle: string | null;
  status: "working" | "needs-input" | "done" | "idle";
  progress?: { task: string; title: string; percent: number; step: string };
  context?: number;
};

const PANES: Record<string, PaneSpec[]> = {
  "ws-web": [
    {
      id: "p-dark",
      provider: "claude",
      title: "orbit-web",
      repo: "orbit-web",
      worktree: wt("orbit-web", "dark-mode", "winfer/dark-mode"),
      agentTitle: "Dark mode for settings",
      status: "working",
      progress: { task: "Add a dark mode toggle to the settings page and remember the choice", title: "Dark mode toggle", percent: 64, step: "Writing the ThemeToggle tests" },
      context: 41,
    },
    {
      id: "p-e2e",
      provider: "codex",
      title: "orbit-web",
      repo: "orbit-web",
      worktree: wt("orbit-web", "checkout-e2e", "winfer/checkout-e2e"),
      agentTitle: "Fix flaky checkout e2e",
      status: "working",
      progress: { task: "The checkout e2e test fails every few runs, find out why and fix it", title: "Flaky checkout e2e", percent: 38, step: "Re-running checkout.spec 20×" },
      context: 18,
    },
    {
      id: "p-stripe",
      provider: "claude",
      title: "orbit-web",
      repo: "orbit-web",
      worktree: wt("orbit-web", "stripe-v2", "winfer/stripe-v2"),
      agentTitle: "Migrate billing to Stripe v2",
      status: "needs-input",
      progress: { task: "Move billing from the Stripe v1 charges API to PaymentIntents", title: "Stripe v2 billing", percent: 72, step: "Waiting to run the database migration" },
      context: 67,
    },
    {
      id: "p-dev",
      provider: "shell",
      title: "orbit-web",
      repo: "orbit-web",
      worktree: null,
      agentTitle: null,
      status: "idle",
    },
  ],
  "ws-api": [
    {
      id: "p-rate",
      provider: "claude",
      title: "orbit-api",
      repo: "orbit-api",
      worktree: wt("orbit-api", "rate-limits", "winfer/rate-limits"),
      agentTitle: "Per-team API rate limits",
      status: "working",
      progress: { task: "Add per-team rate limits to the public API", title: "Team rate limits", percent: 51, step: "Adding the Redis token bucket" },
      context: 33,
    },
    {
      id: "p-n1",
      provider: "codex",
      title: "orbit-api",
      repo: "orbit-api",
      worktree: wt("orbit-api", "invoice-n1", "winfer/invoice-n1"),
      agentTitle: "Fix N+1 in invoice list",
      status: "done",
      progress: { task: "The invoices endpoint is slow for big teams, fix the N+1 queries", title: "Invoice N+1", percent: 100, step: "Done: 1 query instead of 214" },
      context: 22,
    },
    {
      id: "p-docs",
      provider: "opencode",
      title: "orbit-api",
      repo: "orbit-api",
      worktree: wt("orbit-api", "openapi", "winfer/openapi"),
      agentTitle: "OpenAPI docs for v2",
      status: "working",
      progress: { task: "Generate OpenAPI docs for the v2 endpoints", title: "OpenAPI v2 docs", percent: 27, step: "Documenting /v2/invoices" },
    },
  ],
  "ws-mobile": [
    {
      id: "p-push",
      provider: "claude",
      title: "orbit-mobile",
      repo: "orbit-mobile",
      worktree: wt("orbit-mobile", "push-settings", "winfer/push-settings"),
      agentTitle: "Push notification settings",
      status: "working",
      progress: { task: "Let users choose which push notifications they get", title: "Push settings", percent: 45, step: "Building the settings screen" },
      context: 29,
    },
    {
      id: "p-a11y",
      provider: "codex",
      title: "orbit-mobile",
      repo: "orbit-mobile",
      worktree: wt("orbit-mobile", "a11y-labels", "winfer/a11y-labels"),
      agentTitle: "Accessibility labels",
      status: "needs-input",
      progress: { task: "Add accessibility labels to every icon button", title: "A11y labels", percent: 83, step: "Asking which copy to use for the share button" },
      context: 54,
    },
  ],
};

const leaf = (paneId: string) => ({ kind: "leaf", paneId });
const split = (dir: "row" | "col", children: unknown[], sizes?: number[]) => ({ kind: "split", dir, children, sizes: sizes ?? children.map(() => 1 / children.length) });

const LAYOUTS: Record<string, unknown> = {
  "ws-web": split("row", [split("col", [leaf("p-dark"), leaf("p-stripe")], [0.6, 0.4]), split("col", [leaf("p-e2e"), leaf("p-dev")], [0.6, 0.4])]),
  "ws-api": split("row", [leaf("p-rate"), split("col", [leaf("p-n1"), leaf("p-docs")])]),
  "ws-mobile": split("row", [leaf("p-push"), leaf("p-a11y")]),
};

const WORKSPACES = [
  { id: "ws-web", name: "orbit-web", repo: "orbit-web" },
  { id: "ws-api", name: "orbit-api", repo: "orbit-api" },
  { id: "ws-mobile", name: "orbit-mobile", repo: "orbit-mobile" },
];

export const allPanes = () => Object.values(PANES).flat();

export function showcaseState() {
  const theme = SHOWCASE === "light" ? "light" : "dark";
  return {
    workspaces: WORKSPACES.map((w) => ({
      id: w.id,
      name: w.name,
      rootDir: `${ROOT}/${w.repo}`,
      layout: LAYOUTS[w.id],
      zoomedPaneId: null,
      presetIndex: 0,
      useWorktrees: true,
      panes: PANES[w.id].map((p) => ({
        id: p.id,
        provider: p.provider,
        title: p.title,
        cwd: p.worktree?.path ?? `${ROOT}/${p.repo}`,
        worktree: p.worktree,
        raceId: null,
        model: null,
        agentTitle: p.agentTitle,
      })),
    })),
    ui: {
      activeWorkspaceId: "ws-web",
      sidebarOpen: true,
      browserOpen: !NO_BROWSER,
      browserWidth: 520,
      sidePanel: "browser",
      theme,
      // The review inbox covers every workspace from the overview.
      view: SHOWCASE === "overview" || SHOWCASE === "review" ? "overview" : "workspace",
      terminalFontSize: 12,
      defaultProvider: "claude",
      // Keeps the demo progress instead of re-estimating it.
      progress: { enabled: false },
    },
  };
}

// Terminal transcripts -------------------------------------------------------------------------

const E = "\x1b[";
const bold = (s: string) => `${E}1m${s}${E}22m`;
const c = (n: number, s: string) => `${E}38;5;${n}m${s}${E}39m`;
const green = (s: string) => c(78, s);
const red = (s: string) => c(203, s);
const orange = (s: string) => c(173, s);
const blue = (s: string) => c(75, s);
const gray = (s: string) => c(245, s);
const lines = (...l: string[]) => l.join("\r\n") + "\r\n";

const claudeHeader = (dir: string) => lines(`${orange("✻")} ${bold("Claude Code")} ${gray(dir)}`, "");

const TRANSCRIPTS: Record<string, string> = {
  "p-dark":
    lines(
      `${gray(">")} Add a dark mode toggle to the settings page`,
      `  and remember the choice`,
      "",
      `${bold("⏺")} I'll check how theming works first.`,
      "",
      `${green("⏺")} ${bold("Read")}(src/styles/theme.ts)`,
      `  ${gray("⎿  Read 84 lines")}`,
      `${green("⏺")} ${bold("Read")}(src/pages/Settings.tsx)`,
      `  ${gray("⎿  Read 212 lines")}`,
      "",
      `${bold("⏺")} Theme tokens already exist as CSS variables,`,
      `  so a ${blue("data-theme")} attribute on <html> is enough.`,
      "",
      `${green("⏺")} ${bold("Update")}(src/pages/Settings.tsx)`,
      `  ${gray("⎿  24 additions, 3 removals")}`,
      `     ${gray("41")}  <Section title="Appearance">`,
      `     ${gray("42")} ${green("+  <ThemeToggle value={theme}")}`,
      `     ${gray("43")} ${green("+    onChange={setTheme} />")}`,
      `${green("⏺")} ${bold("Write")}(src/components/ThemeToggle.tsx)`,
      `  ${gray("⎿  Wrote 58 lines")}`,
      `${green("⏺")} ${bold("Bash")}(pnpm vitest run theme)`,
      `  ${gray("⎿")}  ${green("✓")} 6 passed ${gray("(412ms)")}`,
      "",
      `${orange("✻")} ${orange("Writing the ThemeToggle tests…")} ${gray("(38s)")}`,
    ),
  "p-stripe":
    lines(
      `${green("⏺")} ${bold("Update")}(src/billing/charge.ts)`,
      `  ${gray("⎿  61 additions, 48 removals")}`,
      `${green("⏺")} ${bold("Write")}(prisma/migrations/stripe_v2.sql)`,
      `  ${gray("⎿  Wrote 22 lines")}`,
      "",
      `${orange("╭──────────────────────────────────────────╮")}`,
      `${orange("│")} ${bold("Bash command")}                             ${orange("│")}`,
      `${orange("│")}   pnpm prisma migrate dev                ${orange("│")}`,
      `${orange("│")}   ${gray("Apply the PaymentIntents migration")}     ${orange("│")}`,
      `${orange("│")}                                          ${orange("│")}`,
      `${orange("│")} Do you want to proceed?                  ${orange("│")}`,
      `${orange("│")} ${blue("❯ 1. Yes")}                                 ${orange("│")}`,
      `${orange("│")}   2. Yes, and don't ask again            ${orange("│")}`,
      `${orange("│")}   3. No, and tell Claude what to do      ${orange("│")}`,
      `${orange("╰──────────────────────────────────────────╯")}`,
    ),
  "p-e2e": lines(
    `${bold(">_ OpenAI Codex")} ${gray("(v0.162.1)")}`,
    gray("model: gpt-6.1-sol   directory: ~/code/orbit-web"),
    "",
    `${gray("›")} The checkout e2e test fails every few runs,`,
    `  find out why and fix it`,
    "",
    `${bold("•")} ${bold("Explored")}`,
    `  ${gray("└")} Read checkout.spec.ts, cart.ts, api/mock.ts`,
    `${bold("•")} ${bold("Ran")} playwright test checkout ×20`,
    `  ${gray("└")} ${red("✗ 3 failed")}, ${green("17 passed")}`,
    `    ${gray("Timeout waiting for [data-testid=pay]")}`,
    "",
    `${bold("•")} The pay button renders before the cart`,
    `  total resolves, so the click lands on a`,
    `  disabled button. Waiting for ${blue("cart.total")}`,
    `  instead of a fixed delay.`,
    "",
    `${bold("•")} ${bold("Edited")} tests/checkout.spec.ts ${green("(+6")} ${red("-2)")}`,
    `${bold("•")} ${bold("Ran")} playwright test checkout ×20`,
    `  ${gray("└")} ${green("✓ 20 passed")} ${gray("(41.2s)")}`,
    "",
    `${blue("◦")} Working ${gray("(1m 12s • esc to interrupt)")}`,
  ),
  "p-dev": lines(
    `${green("➜")}  ${blue("orbit-web")} ${gray("git:(main)")} pnpm dev`,
    "",
    `  ${green(bold("VITE"))} ${green("v7.1.4")}  ready in ${bold("312")} ms`,
    "",
    `  ${green("➜")}  ${bold("Local")}:   ${blue("http://localhost:5173/")}`,
    `  ${gray("➜  Network: use --host to expose")}`,
    "",
    `${gray("10:42:07")} ${blue("[vite]")} ${green("hmr update")} Settings.tsx`,
  ),
  "p-rate":
    claudeHeader("~/code/orbit-api/.worktrees/rate-limits") +
    lines(
      `${green("⏺")} ${bold("Read")}(src/middleware/auth.ts)`,
      `  ${gray("⎿  Read 96 lines")}`,
      `${green("⏺")} ${bold("Write")}(src/middleware/rateLimit.ts)`,
      `  ${gray("⎿  Wrote 74 lines")}`,
      "",
      `${orange("✻")} ${orange("Adding the Redis token bucket…")} ${gray("(1m 4s)")}`,
    ),
  "p-n1": lines(
    `${bold(">_ OpenAI Codex")} ${gray("(v0.162.1)")}`,
    "",
    `${bold("•")} ${bold("Edited")} src/invoices/list.ts ${green("(+9")} ${red("-14)")}`,
    `${bold("•")} ${bold("Ran")} pnpm test invoices`,
    `  ${gray("└")} ${green("✓ 32 passed")}`,
    "",
    `${bold("•")} Invoices now load with one query`,
    `  (was 214 for a 200-member team).`,
    `  p95: 1.8 s → 90 ms.`,
  ),
  "p-docs": lines(`${bold("opencode")} ${gray("v2.0.28")}`, "", `${blue("▣")} Documenting /v2/invoices…`),
  "p-push":
    claudeHeader("~/code/orbit-mobile/.worktrees/push-settings") +
    lines(`${green("⏺")} ${bold("Write")}(app/settings/notifications.tsx)`, `  ${gray("⎿  Wrote 140 lines")}`, "", `${orange("✻")} ${orange("Building the settings screen…")}`),
  "p-a11y": lines(
    `${bold(">_ OpenAI Codex")} ${gray("(v0.162.1)")}`,
    "",
    `${bold("•")} ${bold("Edited")} 23 files ${green("(+46")} ${red("-0)")}`,
    "",
    `${bold("•")} The share button could be labelled`,
    `  "Share" or "Share invoice".`,
    `  Which one do you want?`,
  ),
};

export const transcript = (paneId: string) => TRANSCRIPTS[paneId];

// Browser page ---------------------------------------------------------------------------------

/** The app the agents are building, in the browser panel: Orbit's settings page. */
export function drawApp(ctx: OffscreenCanvasRenderingContext2D, width: number, height: number) {
  const font = (w: number, size: number) => `${w} ${size}px Inter, system-ui, sans-serif`;
  const round = (x: number, y: number, w: number, h: number, r: number, fill: string) => {
    ctx.fillStyle = fill;
    ctx.beginPath();
    ctx.roundRect(x, y, w, h, r);
    ctx.fill();
  };
  ctx.fillStyle = "#0f1117";
  ctx.fillRect(0, 0, width, height);
  // Nav
  ctx.fillStyle = "#161923";
  ctx.fillRect(0, 0, width, 56);
  round(20, 16, 24, 24, 7, "#7c6cff");
  ctx.fillStyle = "#e8e8f0";
  ctx.font = font(650, 17);
  ctx.fillText("Orbit", 54, 34);
  ctx.font = font(500, 14);
  ctx.fillStyle = "#8b8fa3";
  ["Dashboard", "Invoices", "Team"].forEach((t, i) => ctx.fillText(t, 130 + i * 92, 34));
  ctx.fillStyle = "#e8e8f0";
  ctx.fillText("Settings", 130 + 3 * 92, 34);
  round(width - 46, 14, 28, 28, 14, "#2a2f45");
  // Title
  ctx.fillStyle = "#f2f2f7";
  ctx.font = font(700, 26);
  ctx.fillText("Settings", 28, 108);
  ctx.fillStyle = "#8b8fa3";
  ctx.font = font(400, 14);
  ctx.fillText("Manage your account and how Orbit looks.", 28, 134);
  // Appearance card
  const card = (y: number, h: number) => {
    round(28, y, width - 56, h, 12, "#161923");
    ctx.strokeStyle = "#252a3a";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.roundRect(28.5, y + 0.5, width - 57, h - 1, 12);
    ctx.stroke();
  };
  card(160, 172);
  ctx.fillStyle = "#f2f2f7";
  ctx.font = font(600, 16);
  ctx.fillText("Appearance", 48, 192);
  ctx.fillStyle = "#8b8fa3";
  ctx.font = font(400, 13);
  ctx.fillText("Choose a theme. Saved on this device.", 48, 214);
  const opts = ["Light", "Dark", "System"];
  const w = Math.min(140, (width - 120) / 3);
  opts.forEach((t, i) => {
    const x = 48 + i * (w + 12);
    const active = t === "Dark";
    round(x, 234, w, 78, 10, active ? "#221f3d" : "#11141c");
    ctx.strokeStyle = active ? "#7c6cff" : "#252a3a";
    ctx.lineWidth = active ? 2 : 1;
    ctx.beginPath();
    ctx.roundRect(x + 1, 235, w - 2, 76, 10);
    ctx.stroke();
    round(x + 14, 248, w - 28, 26, 5, t === "Light" ? "#e9e9f0" : t === "Dark" ? "#0b0d12" : "#5b5f73");
    ctx.fillStyle = active ? "#c8c2ff" : "#a3a7b8";
    ctx.font = font(active ? 600 : 500, 13);
    ctx.fillText(t, x + 14, 298);
  });
  // Notifications card
  card(348, 150);
  ctx.fillStyle = "#f2f2f7";
  ctx.font = font(600, 16);
  ctx.fillText("Notifications", 48, 380);
  [
    ["Invoice paid", true],
    ["Weekly summary", true],
    ["Product updates", false],
  ].forEach(([label, on], i) => {
    const y = 412 + i * 30;
    ctx.fillStyle = "#c9cbd6";
    ctx.font = font(400, 14);
    ctx.fillText(label as string, 48, y + 5);
    round(width - 92, y - 10, 36, 20, 10, on ? "#7c6cff" : "#2a2f45");
    round(on ? width - 74 : width - 90, y - 8, 16, 16, 8, "#ffffff");
  });
}

// Seeding the stores ---------------------------------------------------------------------------

/** Statuses, progress, context, reviews and history, after the app has loaded its state. */
export async function seedShowcase() {
  const [{ useWorkspaces }, { useProgress }, { useUsage }, { useReviews }, browser] = await Promise.all([
    import("@/stores/workspaces"),
    import("@/features/progress/progress"),
    import("@/features/usage/usage"),
    import("@/features/review/reviews"),
    import("@/commands/bus"),
  ]);
  const now = Date.now();
  const ws = useWorkspaces.getState();
  for (const p of allPanes()) ws.setPaneStatus(p.id, p.status);
  ws.focusPane("p-dark");
  useWorkspaces.setState((s) => ({
    workspaces: s.workspaces.map((w) => ({ ...w, panes: w.panes.map((p) => ({ ...p, unseen: false, devUrl: p.id === "p-dev" ? "http://localhost:5173/" : p.devUrl })) })),
  }));
  useProgress.setState({
    byPane: Object.fromEntries(
      allPanes()
        .filter((p) => p.progress)
        .map((p) => [p.id, { ...p.progress!, updatedAt: now - 40_000, estimating: false, error: null }]),
    ),
  });
  useUsage.setState({
    context: Object.fromEntries(
      allPanes()
        .filter((p) => p.context)
        .map((p) => [p.id, { usedPercent: p.context!, tokens: Math.round((p.provider === "codex" ? 475_000 : 200_000) * p.context! / 100), window: p.provider === "codex" ? 475_000 : 200_000, at: now, source: "mock.jsonl" }]),
    ),
  });
  useReviews.setState({
    reviews: [
      {
        id: "r-n1",
        paneId: "p-n1",
        workspaceId: "ws-api",
        provider: "codex",
        raceId: null,
        prompt: "The invoices endpoint is slow for big teams, fix the N+1 queries",
        finishedAt: now - 4 * 60_000,
        branch: "winfer/invoice-n1",
        dir: wt("orbit-api", "invoice-n1").path,
        stats: { files: ["src/invoices/list.ts", "src/invoices/list.test.ts", "prisma/schema.prisma"], adds: 41, dels: 26, ahead: 2, dirty: false },
        lastOutput: "",
        summary: { state: "done", value: "Loads invoices with their line items and customers in one query instead of one per invoice (214 → 1 for a 200-member team; p95 1.8 s → 90 ms). Adds a test that fails if the query count grows again." },
        tests: { state: "done", value: { command: "pnpm test invoices", ok: true, output: "✓ 32 passed (2.4s)", ms: 2400 } },
        testCommand: "pnpm test invoices",
        screenshot: { state: "idle" },
        status: "open",
      },
      {
        id: "r-dark",
        paneId: "p-dark",
        workspaceId: "ws-web",
        provider: "claude",
        raceId: null,
        prompt: "Add a dark mode toggle to the settings page and remember the choice",
        finishedAt: now - 11 * 60_000,
        branch: "winfer/dark-mode",
        dir: wt("orbit-web", "dark-mode").path,
        stats: { files: ["src/pages/Settings.tsx", "src/components/ThemeToggle.tsx", "src/components/ThemeToggle.test.tsx", "src/styles/theme.ts"], adds: 128, dels: 9, ahead: 3, dirty: false },
        lastOutput: "",
        summary: { state: "done", value: "Adds Light / Dark / System to Settings → Appearance. The choice is saved in localStorage and applied before first paint, so there's no flash of the wrong theme. Six new tests cover the toggle and the saved value." },
        tests: { state: "done", value: { command: "pnpm vitest run", ok: true, output: "✓ 214 passed (8.1s)", ms: 8100 } },
        testCommand: "pnpm vitest run",
        screenshot: { state: "done", value: appShot(), url: "http://localhost:5173/settings" },
        status: "open",
      },
    ],
  });
  // The browser panel shows the app the agents are working on.
  if (NO_BROWSER) return;
  browser.execute({ type: "browser.open", url: "http://localhost:5173/settings" });
}

/** The settings page as a review card screenshot. */
function appShot(): string {
  const canvas = new OffscreenCanvas(640, 400);
  drawApp(canvas.getContext("2d")!, 640, 400);
  const c = document.createElement("canvas");
  c.width = 640;
  c.height = 400;
  c.getContext("2d")!.drawImage(canvas.transferToImageBitmap(), 0, 0);
  return c.toDataURL("image/jpeg", 0.85);
}

/** Opens what each scene shows, once the app has settled. */
export async function stageScene() {
  const { useWorkspaces } = await import("@/stores/workspaces");
  const s = useWorkspaces.getState();
  if (SHOWCASE === "review") s.setOverlay("review");
  if (SHOWCASE === "sessions") s.setOverlay("progress");
  if (SHOWCASE === "usage") {
    const meter = [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.title.includes(" left"));
    meter?.click();
  }
  if (SHOWCASE === "overview") s.setActiveWorkspace("ws-web");
  const page = PAGE_SCENES[SHOWCASE ?? ""];
  if (page) {
    s.setView(page as `page:${string}`);
    // Let the page load its data (and the graph settle) before the capture.
    await new Promise((r) => setTimeout(r, 2500));
  }
  document.documentElement.dataset.showcaseReady = "1";
}
