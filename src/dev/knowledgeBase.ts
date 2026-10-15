/**
 * Dev-only fake graphify for the Knowledge Base plugin (plugins/knowledge-base): answers the shell
 * commands the plugin runs through `vault_exec` with a generated graph of a made-up "orbit" web
 * app, whose files match the showcase agents' transcripts. Open the dev server with `?kb` to
 * install the plugin, or `?kb=empty` to start without a graph.
 */

type Node = { id: string; label: string; file_type: string; source_file: string; source_location: string; community: number };
type Link = { source: string; target: string; relation: string; confidence: string; weight: number };

const MODE = new URLSearchParams(location.search).get("kb");
export const KB_DEMO = MODE !== null;

function rng(seed: number) {
  return () => {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const AREAS: { name: string; files: string[]; nouns: string[] }[] = [
  { name: "Billing & Stripe Checkout", files: ["src/billing/charge.ts", "src/billing/stripe.ts", "src/billing/webhooks.ts", "src/billing/plans.ts", "src/cart/cart.ts", "src/billing/coupons.ts"], nouns: ["Charge", "Invoice", "Plan", "Coupon", "Cart", "Price", "Webhook", "Refund", "Customer", "Checkout"] },
  { name: "Settings & Theming", files: ["src/pages/Settings.tsx", "src/styles/theme.ts", "src/components/ThemeToggle.tsx", "src/hooks/useTheme.ts", "src/pages/Profile.tsx"], nouns: ["Theme", "Palette", "Preference", "Profile", "Avatar", "Locale", "Toggle", "Mode"] },
  { name: "Invoices & PDFs", files: ["src/invoices/list.ts", "src/invoices/pdf.ts", "src/invoices/InvoiceTable.tsx", "src/invoices/filters.ts"], nouns: ["Invoice", "Row", "Pdf", "Filter", "Page", "Total", "Column"] },
  { name: "Auth & Sessions", files: ["src/middleware/auth.ts", "src/auth/session.ts", "src/auth/oauth.ts", "src/auth/LoginForm.tsx", "src/auth/tokens.ts"], nouns: ["Session", "Token", "User", "Login", "Provider", "Claim", "Cookie"] },
  { name: "API Router & Middleware", files: ["src/middleware/rateLimit.ts", "src/middleware/logger.ts", "src/api/router.ts", "src/api/mock.ts", "src/api/errors.ts"], nouns: ["Route", "Request", "Limit", "Bucket", "Error", "Handler", "Log"] },
  { name: "Data Layer (Prisma)", files: ["src/db/client.ts", "src/db/users.ts", "src/db/subscriptions.ts", "prisma/schema.prisma", "prisma/migrations/stripe_v2.sql"], nouns: ["Query", "User", "Subscription", "Transaction", "Migration", "Record"] },
  { name: "Design System Components", files: ["src/components/Button.tsx", "src/components/Card.tsx", "src/components/Modal.tsx", "src/components/Toast.tsx", "src/components/Table.tsx"], nouns: ["Button", "Card", "Modal", "Toast", "Table", "Icon", "Slot"] },
  { name: "Notifications", files: ["app/settings/notifications.tsx", "src/notifications/push.ts", "src/notifications/email.ts", "src/notifications/templates.ts"], nouns: ["Notification", "Push", "Email", "Template", "Channel", "Digest"] },
  { name: "End-to-end Tests", files: ["tests/checkout.spec.ts", "tests/settings.spec.ts", "tests/fixtures.ts", "src/invoices/list.test.ts", "src/components/ThemeToggle.test.tsx"], nouns: ["Fixture", "Spec", "Page", "Mock", "Step"] },
  { name: "Analytics & Events", files: ["src/analytics/track.ts", "src/analytics/events.ts", "src/analytics/funnels.ts"], nouns: ["Event", "Funnel", "Metric", "Track"] },
  { name: "Search", files: ["src/search/index.ts", "src/search/SearchBar.tsx", "src/search/rank.ts"], nouns: ["Index", "Result", "Query", "Rank"] },
  { name: "Feature Flags & Config", files: ["src/flags/flags.ts", "src/config/env.ts"], nouns: ["Flag", "Env", "Config"] },
];
const DOCS = [
  { file: "docs/architecture.md", label: "Architecture Overview" },
  { file: "docs/billing.md", label: "Billing Guide" },
  { file: "docs/adr/0003-stripe-v2.md", label: "ADR 3: Move to Stripe API v2" },
  { file: "docs/adr/0005-rate-limits.md", label: "ADR 5: Token Bucket Rate Limits" },
  { file: "docs/onboarding.md", label: "Onboarding Checklist" },
  { file: "README.md", label: "README" },
];
const VERBS = ["get", "load", "save", "build", "parse", "format", "validate", "compute", "render", "use", "handle", "sync", "fetch", "normalize", "ensure", "create", "update", "apply"];

interface Fake {
  nodes: Node[];
  links: Link[];
  names: string[];
  hyper: { id: string; label: string; nodes: string[]; relation: string; confidence: string }[];
  stamp: number;
  commit: string;
}

let fake: Fake | null = null;
let exists = MODE !== "empty";
let generation = 0;

function generate(extra: number): Fake {
  const rand = rng(42);
  const pick = <T,>(list: T[]) => list[Math.floor(rand() * list.length)];
  const nodes: Node[] = [];
  const links: Link[] = [];
  const byArea: string[][] = [];
  const fileNode = new Map<string, string>();
  const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "");
  const link = (source: string, target: string, relation: string, confidence = "EXTRACTED") => {
    if (source !== target) links.push({ source, target, relation, confidence, weight: 1 });
  };

  AREAS.forEach((area, c) => {
    const ids: string[] = [];
    for (const file of area.files) {
      const fid = slug(file);
      nodes.push({ id: fid, label: file.split("/").pop()!, file_type: "code", source_file: file, source_location: "L1", community: c });
      fileNode.set(file, fid);
      ids.push(fid);
      const count = 8 + Math.floor(rand() * 18);
      const used = new Set<string>();
      for (let k = 0; k < count; k++) {
        const noun = pick(area.nouns);
        const component = file.endsWith(".tsx") && rand() < 0.3;
        const label = component ? `${noun}${pick(["View", "Panel", "Row", "Header", "List"])}` : rand() < 0.15 ? `${noun}${pick(["Service", "Store", "Client", "Schema"])}` : `${pick(VERBS)}${noun}${rand() < 0.3 ? pick(["s", "ById", "ForUser", "Async"]) : ""}()`;
        if (used.has(label)) continue;
        used.add(label);
        const id = `${fid}_${slug(label)}`;
        nodes.push({ id, label, file_type: "code", source_file: file, source_location: `L${10 + Math.floor(rand() * 400)}`, community: c });
        link(fid, id, "contains");
        ids.push(id);
      }
    }
    byArea.push(ids);
  });

  // Calls inside an area, imports between files, and a few calls into other areas' hubs.
  byArea.forEach((ids, c) => {
    for (const id of ids) {
      const calls = 1 + Math.floor(rand() * 3);
      for (let k = 0; k < calls; k++) link(id, pick(ids), "calls", rand() < 0.12 ? "INFERRED" : "EXTRACTED");
      if (rand() < 0.1) {
        const other = (c + 1 + Math.floor(rand() * (AREAS.length - 1))) % AREAS.length;
        link(id, byArea[other][1 + Math.floor(rand() * Math.min(4, byArea[other].length - 1))], rand() < 0.7 ? "calls" : "uses", rand() < 0.25 ? "INFERRED" : "EXTRACTED");
      }
    }
    for (const file of AREAS[c].files) {
      const imports = 1 + Math.floor(rand() * 2);
      for (let k = 0; k < imports; k++) {
        const target = rand() < 0.75 ? pick(AREAS[c].files) : pick(pick(AREAS).files);
        link(fileNode.get(file)!, fileNode.get(target)!, "imports");
      }
    }
  });

  // Docs, rationale and concepts.
  const docArea = AREAS.length;
  for (const doc of DOCS) {
    const id = slug(doc.file);
    nodes.push({ id, label: doc.label, file_type: "document", source_file: doc.file, source_location: "L1", community: docArea });
    for (let k = 0; k < 4; k++) link(id, pick(pick(byArea)), "references");
  }
  const concepts = [
    ["Idempotent Webhook Handling", 0],
    ["Token Bucket Rate Limiting", 4],
    ["Dark Mode via CSS Variables", 1],
    ["Server-side Session Rotation", 3],
    ["N+1 Query Avoidance", 5],
    ["Optimistic UI Updates", 6],
  ] as const;
  concepts.forEach(([label, area], k) => {
    const id = `concept_${slug(label)}`;
    nodes.push({ id, label, file_type: k % 2 ? "rationale" : "concept", source_file: pick(DOCS).file, source_location: "L1", community: docArea + 1 });
    for (let j = 0; j < 3; j++) link(id, pick(byArea[area]), k % 2 ? "rationale_for" : "conceptually_related_to", "INFERRED");
    if (k === 2) link(id, pick(byArea[2]), "semantically_similar_to", "AMBIGUOUS");
  });
  link(slug("docs/adr/0003-stripe-v2.md"), fileNode.get("prisma/migrations/stripe_v2.sql")!, "rationale_for", "INFERRED");
  link(slug("docs/adr/0005-rate-limits.md"), fileNode.get("src/middleware/rateLimit.ts")!, "rationale_for", "INFERRED");

  for (let k = 0; k < extra; k++) {
    const id = `src_billing_proration_ts_step${k}`;
    nodes.push({ id, label: `${pick(VERBS)}Proration${k ? k : ""}()`, file_type: "code", source_file: "src/billing/proration.ts", source_location: `L${12 + k * 9}`, community: 0 });
    link(id, pick(byArea[0]), "calls");
    link(pick(byArea[0]), id, "calls");
  }

  const names = [...AREAS.map((a) => a.name), "Docs & Decisions", "Concepts & Rationale"];
  const hyper = [
    { id: "h_checkout", label: "Checkout Flow", nodes: [fileNode.get("src/cart/cart.ts")!, fileNode.get("src/billing/charge.ts")!, fileNode.get("src/billing/stripe.ts")!, fileNode.get("tests/checkout.spec.ts")!], relation: "participate_in", confidence: "EXTRACTED" },
    { id: "h_theme", label: "Theme Switching", nodes: [fileNode.get("src/styles/theme.ts")!, fileNode.get("src/components/ThemeToggle.tsx")!, fileNode.get("src/hooks/useTheme.ts")!], relation: "implement", confidence: "EXTRACTED" },
    { id: "h_auth", label: "Request Authentication", nodes: [fileNode.get("src/middleware/auth.ts")!, fileNode.get("src/auth/session.ts")!, fileNode.get("src/auth/tokens.ts")!, fileNode.get("src/api/router.ts")!], relation: "implement", confidence: "INFERRED" },
    { id: "h_invoices", label: "Invoice Export", nodes: [fileNode.get("src/invoices/list.ts")!, fileNode.get("src/invoices/pdf.ts")!, fileNode.get("src/db/subscriptions.ts")!], relation: "form", confidence: "INFERRED" },
  ];
  return { nodes, links, names, hyper, stamp: Math.floor(Date.now() / 1000), commit: "9f3c2a71e0b4d5c6a7e8f9a0b1c2d3e4f5a6b7c8" };
}

function current(): Fake {
  fake ??= generate(0);
  return fake;
}

function graphJson(f: Fake, nodes = f.nodes) {
  const keep = new Set(nodes.map((n) => n.id));
  return JSON.stringify({
    directed: true,
    multigraph: false,
    graph: {},
    nodes,
    links: f.links.filter((l) => keep.has(l.source) && keep.has(l.target)),
    hyperedges: f.hyper,
    built_at_commit: f.commit,
  });
}

function degree(f: Fake) {
  const d = new Map<string, number>();
  for (const l of f.links) {
    d.set(l.source, (d.get(l.source) ?? 0) + 1);
    d.set(l.target, (d.get(l.target) ?? 0) + 1);
  }
  return d;
}

function report(f: Fake) {
  const d = degree(f);
  const god = [...f.nodes].sort((a, b) => (d.get(b.id) ?? 0) - (d.get(a.id) ?? 0)).slice(0, 10);
  const byId = new Map(f.nodes.map((n) => [n.id, n]));
  const cross = f.links.filter((l) => byId.get(l.source)!.community !== byId.get(l.target)!.community && l.relation !== "contains").slice(0, 5);
  const extracted = f.links.filter((l) => l.confidence === "EXTRACTED").length;
  return [
    "# Graph Report - orbit-web  (2026-10-14)",
    "",
    "## Corpus Check",
    `- ${AREAS.reduce((n, a) => n + a.files.length, 0) + DOCS.length} files · ~184,220 words`,
    "",
    "## Summary",
    `- ${f.nodes.length} nodes · ${f.links.length} edges · ${f.names.length} communities`,
    `- Extraction: ${Math.round((extracted / f.links.length) * 100)}% EXTRACTED · ${100 - Math.round((extracted / f.links.length) * 100)}% INFERRED · 0% AMBIGUOUS`,
    "",
    "## Graph Freshness",
    `- Built from commit: \`${f.commit.slice(0, 8)}\``,
    "",
    "## God Nodes (most connected - your core abstractions)",
    ...god.map((n, i) => `${i + 1}. \`${n.label}\` - ${d.get(n.id)} edges`),
    "",
    "## Surprising Connections (you probably didn't know these)",
    ...cross.flatMap((l) => {
      const a = byId.get(l.source)!;
      const b = byId.get(l.target)!;
      return [`- \`${a.label}\` --${l.relation}--> \`${b.label}\`  [${l.confidence}]`, `  ${a.source_file} → ${b.source_file}`];
    }),
    "",
    "## Hyperedges (group relationships)",
    ...f.hyper.map((x) => `- **${x.label}** — ${x.nodes.join(", ")} [${x.confidence} 1.00]`),
    "",
    `## Communities (${f.names.length} total, 0 thin omitted)`,
    "",
    ...f.names.flatMap((name, i) => [`### Community ${i} - "${name}"`, `Cohesion: ${(0.08 + ((i * 37) % 60) / 100).toFixed(2)}`, `Nodes (${f.nodes.filter((n) => n.community === i).length}): …`, ""]),
  ].join("\n");
}

const SNAPSHOTS = ["2026-09-02", "2026-09-09", "2026-09-16", "2026-09-23", "2026-09-30", "2026-10-07", "2026-10-07_2"];

function nodeLine(n: Node) {
  return `NODE ${n.label} [src=${n.source_file} loc=${n.source_location} community=${n.community}]`;
}

function unquote(args: string): string[] {
  return [...args.matchAll(/'((?:[^']|'\\'')*)'/g)].map((m) => m[1].replace(/'\\''/g, "'"));
}

function query(f: Fake, question: string) {
  const words = question.toLowerCase().split(/\W+/).filter((w) => w.length > 2);
  const d = degree(f);
  const scored = f.nodes
    .map((n) => [n, words.filter((w) => `${n.label} ${n.source_file}`.toLowerCase().includes(w)).length] as const)
    .filter(([, s]) => s > 0)
    .sort((a, b) => b[1] - a[1] || (d.get(b[0].id) ?? 0) - (d.get(a[0].id) ?? 0));
  const start = scored.slice(0, 3).map(([n]) => n);
  if (!start.length) return "No matching nodes found.";
  const found = new Map(start.map((n) => [n.id, n]));
  const byId = new Map(f.nodes.map((n) => [n.id, n]));
  for (const l of f.links) {
    if (found.size >= 28) break;
    if (found.has(l.source) && !found.has(l.target)) found.set(l.target, byId.get(l.target)!);
    else if (found.has(l.target) && !found.has(l.source)) found.set(l.source, byId.get(l.source)!);
  }
  return [`Traversal: BFS depth=2 | Start: [${start.map((n) => `'${n.label}'`).join(", ")}] | ${found.size} nodes found`, "", ...[...found.values()].map(nodeLine)].join("\n");
}

function explain(f: Fake, label: string) {
  const n = f.nodes.find((x) => x.label === label);
  if (!n) return `No node matching '${label}'.`;
  const byId = new Map(f.nodes.map((x) => [x.id, x]));
  const conns = f.links.filter((l) => l.source === n.id || l.target === n.id);
  return [
    `Node: ${n.label}`,
    `  ID:        ${n.id}`,
    `  Source:    ${n.source_file} ${n.source_location}`,
    `  Type:      ${n.file_type}`,
    `  Community: ${n.community}`,
    `  Degree:    ${conns.length}`,
    "",
    `Connections (${conns.length}):`,
    ...conns.map((l) => (l.source === n.id ? `  --> ${byId.get(l.target)!.label} [${l.relation}] [${l.confidence}]` : `  <-- ${byId.get(l.source)!.label} [${l.relation}] [${l.confidence}]`)),
  ].join("\n");
}

const delay = <T,>(ms: number, value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), ms));
const ok = (output: string) => ({ code: 0, timedOut: false, output });

// The plugin reads big files gzipped and base64-encoded in chunks (vault_exec keeps only the
// last 20,000 characters); these play the temp files it reads them from.
const tempFiles = new Map<string, string>();
let tempCounter = 0;

function fileText(rel: string): string | null {
  const f = current();
  if (rel === "graphify-out/graph.json") return graphJson(f);
  if (rel === "graphify-out/GRAPH_REPORT.md") return report(f);
  const snapshot = /^graphify-out\/(\d{4}-\d{2}-\d{2}(?:_\d+)?)\/graph\.json$/.exec(rel);
  if (!snapshot) return null;
  const k = SNAPSHOTS.indexOf(snapshot[1]);
  const share = (k + 1) / (SNAPSHOTS.length + 1);
  return graphJson(f, f.nodes.filter((n, i) => (n.file_type === "code" ? i / f.nodes.length < share * 1.1 : k >= 3)));
}

async function gzipBase64(text: string) {
  const stream = new Blob([text]).stream().pipeThrough(new CompressionStream("gzip"));
  const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

/** Answers one `vault_exec` call of the knowledge-base plugin. */
export function knowledgeBaseExec(command: string): Promise<{ code: number; timedOut: boolean; output: string }> | { code: number; timedOut: boolean; output: string } {
  if (command.startsWith("command -v graphify")) return ok("/Users/maya/.local/bin/graphify\ngraphify 0.8.18\n");
  if (command === 'echo "$HOME"') return ok("/Users/maya\n");
  if (/^\[ -f '[^']+' \] && echo @@YES \|\| echo @@NO$/.test(command)) return ok(exists ? "@@YES\n" : "@@NO\n");
  const noGraph = command.includes("@@NOGRAPH") && !exists;
  if (noGraph) return ok("@@NOGRAPH\n");
  if (command.includes("@@NONE")) {
    if (!exists) return delay(300, ok("@@NONE\n"));
    const f = current();
    return delay(200, ok([`@@STAMP ${f.stamp}`, `@@HEAD ${f.commit}`, "@@IGNORED 1", "@@SNAPSHOTS", ...SNAPSHOTS, "GRAPH_REPORT.md", "cache", "graph.html", "graph.json"].join("\n")));
  }
  if (command.startsWith("date -r")) return ok(exists ? `${current().stamp}\n` : "");
  if (command.startsWith("graphify update") || command.startsWith("graphify extract")) {
    const first = !exists;
    exists = true;
    generation += first ? 0 : 1;
    fake = generate(Math.min(18, generation * 6));
    fake.stamp += generation;
    return delay(first ? 2600 : 1800, ok(`Re-extracting code files in . (no LLM needed)...\n[graphify watch] Rebuilt: ${fake.nodes.length} nodes, ${fake.links.length} edges, ${fake.names.length} communities\n`));
  }
  const prepare = /^f=\$\(mktemp\) && gzip -c '([^']+)'/.exec(command);
  if (prepare) {
    const text = exists ? fileText(prepare[1]) : null;
    if (text === null) return { code: 1, timedOut: false, output: `gzip: ${prepare[1]}: No such file or directory\n` };
    return gzipBase64(text).then((b64) => {
      const path = `/tmp/tmp.kb${++tempCounter}`;
      tempFiles.set(path, b64);
      return delay(120, ok(`${b64.length} ${path}\n`));
    });
  }
  const chunk = /^tail -c \+(\d+) '([^']+)' \| head -c (\d+)$/.exec(command);
  if (chunk) {
    const start = Number(chunk[1]) - 1;
    return delay(40, ok((tempFiles.get(chunk[2]) ?? "").slice(start, start + Number(chunk[3]))));
  }
  const remove = /^rm -f '([^']+)'$/.exec(command);
  if (remove) {
    tempFiles.delete(remove[1]);
    return ok("");
  }
  const f = current();
  const args = unquote(command);
  if (command.includes("graphify query")) return delay(500, ok(query(f, args[0] ?? "")));
  if (command.includes("graphify explain")) return delay(200, ok(explain(f, args[0] ?? "")));
  if (command.includes("GRAPH_REPORT.md") && command.includes("sed ")) return ok(`@@HEAD ${f.commit}\n${report(f).split("\n").slice(0, 16).join("\n")}`);
  return ok("");
}
