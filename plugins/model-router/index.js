// Model Router: keeps the expensive model for work that needs it.
//
// - Delegation: every Claude Code pane starts with cheaper subagents (`--agents`): a read-only
//   "scout" for searching and reading, and a "worker" for well-specified mechanical edits, plus a
//   system prompt telling the main model to hand that work to them.
// - Spawn routing: panes started by other agents (spawn_agent) or by a workflow run on the
//   cheapest tier the task needs. The tier comes from the caller's hint, else the rules, else a
//   quick LLM verdict, else "standard"; each tier maps to a model and effort per provider.
//
// Panes the user starts keep the model they picked; only the subagents are added.
//
// Workflows: a planner (frontier tier) splits a goal into tasks with workflow_plan; each task runs
// in its own agent and worktree on the tier it needs (at most `maxParallel` at once, after the
// tasks it depends on), mirrored on the Task Board when that plugin is on; once all are done a
// reviewer checks the branches and either adds follow-up tasks or finishes the workflow.

const STORAGE = "winfer.plugin.model-router";
const MAX_LOG = 200;

export const TIERS = ["cheap", "standard", "frontier"];

/** Effort levels per provider, lowest first (mirrors `efforts` in providers.json). */
export const EFFORTS = {
  claude: ["low", "medium", "high", "xhigh", "max"],
  codex: ["minimal", "low", "medium", "high", "xhigh"],
};

/** Claude Code model aliases from cheapest to most capable, to tell a session's tier. */
const CLAUDE_RANK = ["haiku", "sonnet", "opus", "fable"];

/** First match wins, so the most demanding rules come first. Patterns are case-insensitive. */
export const DEFAULT_RULES = [
  {
    id: "frontier",
    tier: "frontier",
    pattern:
      "\\b(architect\\w*|design (a|the|an)\\b|redesign|security|vulnerab\\w*|race condition|concurren\\w*|deadlock|migrat\\w*|root cause|flaky|performance|optimi[sz]\\w*|across the (code ?base|repo)|plan (the|a|an)\\b|review)\\b",
    note: "Design, hard debugging, security, cross-cutting changes",
  },
  {
    id: "standard",
    tier: "standard",
    pattern:
      "\\b(rename|refactor|format\\w*|lint\\w*|typo|bump|upgrade|boilerplate|scaffold\\w*|add (a )?tests?|write (unit )?tests?|fix (the )?(types?|lint|imports?|build)|docstring|comment|translate|convert)\\b",
    note: "Well-specified, mechanical edits",
  },
  {
    id: "cheap",
    tier: "cheap",
    pattern:
      "^(?![\\s\\S]*\\b(fix|implement|add|change|update|edit|write|create|delete|remove|refactor|make)\\b)\\s*(find|search|grep|look ?up|locate|list|show|count|check (if|whether)|where (is|are|do|does)|which files?|what (is|are|does)|explain|summari[sz]e|read|run (the )?(tests?|linter|build))\\b",
    note: "Searching, reading, listing, summarizing (nothing that edits)",
  },
];

export const DEFAULT_POLICY = {
  /** Give Claude Code panes cheap subagents to delegate to. */
  delegation: true,
  /** Pick model and effort for panes started by agents and workflows. */
  routeSpawns: true,
  /** Ask an LLM when no rule matches (else the work counts as "standard"). */
  llmFallback: true,
  /** Per tier and provider: model ("" keeps the CLI's default) and effort ("" keeps its default). */
  tiers: {
    cheap: { claude: { model: "haiku", effort: "low" }, codex: { model: "", effort: "low" } },
    standard: { claude: { model: "sonnet", effort: "medium" }, codex: { model: "", effort: "medium" } },
    frontier: { claude: { model: "opus", effort: "high" }, codex: { model: "", effort: "high" } },
  },
  rules: DEFAULT_RULES,
};

const text = (v) => (typeof v === "string" ? v.trim() : "");
const clone = (v) => JSON.parse(JSON.stringify(v));

/** Fills missing parts of a saved policy with the defaults; drops broken rules. */
export function normalizePolicy(raw) {
  const p = raw && typeof raw === "object" ? raw : {};
  const tiers = clone(DEFAULT_POLICY.tiers);
  for (const tier of TIERS) {
    for (const [provider, choice] of Object.entries(p.tiers?.[tier] ?? {})) {
      if (choice && typeof choice === "object") tiers[tier][provider] = { model: text(choice.model), effort: text(choice.effort) };
    }
  }
  const rules = Array.isArray(p.rules)
    ? p.rules.filter((r) => r && TIERS.includes(r.tier) && text(r.pattern) && compile(r.pattern)).map((r, i) => ({ id: text(r.id) || `r${i}`, tier: r.tier, pattern: text(r.pattern), note: text(r.note) }))
    : clone(DEFAULT_RULES);
  const flag = (key) => (typeof p[key] === "boolean" ? p[key] : DEFAULT_POLICY[key]);
  return { delegation: flag("delegation"), routeSpawns: flag("routeSpawns"), llmFallback: flag("llmFallback"), tiers, rules };
}

function compile(pattern) {
  try {
    return new RegExp(pattern, "i");
  } catch {
    return null;
  }
}

/** One tier up, for long or many-part work that a rule called simpler. */
const bump = (tier) => TIERS[Math.min(TIERS.indexOf(tier) + 1, TIERS.length - 1)];

/** Many steps or a lot of text: more than a quick lookup. */
export const isBig = (task) => task.length > 600 || (task.match(/^\s*(?:[-*•]|\d+[.)])\s+/gm) ?? []).length >= 4;

/** The tier the first matching rule names: `{ tier, rule }`, or null. Big tasks go one tier up. */
export function classifyRules(task, rules = DEFAULT_RULES) {
  const t = text(task);
  if (!t) return null;
  for (const rule of rules) {
    if (compile(rule.pattern)?.test(t)) return { tier: isBig(t) ? bump(rule.tier) : rule.tier, rule: rule.id };
  }
  return null;
}

export const LLM_SYSTEM = `You route coding-agent tasks to the cheapest model that can do them well.
Reply with JSON only: {"tier":"cheap|standard|frontier","reason":"<10 words"}
cheap: searching, grepping, reading or listing files, summarizing code, running a command and reporting.
standard: clear, well-scoped edits: renames, tests for given behavior, small features, fixing type or lint errors.
frontier: design, debugging an unknown cause, security, concurrency, changes across many files, vague goals.`;

/** The tier in an LLM's reply (JSON, possibly wrapped in prose or a code fence, or a bare word). */
export function parseLlmVerdict(reply) {
  const s = String(reply ?? "");
  const json = s.match(/\{[\s\S]*?\}/);
  if (json) {
    try {
      const v = JSON.parse(json[0]);
      const tier = text(v.tier).toLowerCase();
      if (TIERS.includes(tier)) return { tier, reason: text(v.reason) };
    } catch {
      // Not JSON after all; try a bare word below.
    }
  }
  const word = s.toLowerCase().match(/\b(cheap|standard|frontier)\b/);
  return word ? { tier: word[1], reason: "" } : null;
}

/** Model and effort for a tier on a provider (empty parts become null: the CLI's default). */
export function resolveTier(policy, provider, tier) {
  const choice = policy.tiers[tier]?.[provider];
  if (!choice) return null;
  return { model: choice.model || null, effort: choice.effort || null };
}

/** Rank of a Claude model alias or id (null: the CLI's default, taken as the top). */
export function claudeRank(model) {
  if (!model) return CLAUDE_RANK.length;
  const i = CLAUDE_RANK.findIndex((name) => String(model).toLowerCase().includes(name));
  return i < 0 ? CLAUDE_RANK.length : i;
}

const SCOUT_PROMPT = `You are a fast read-only scout. Find what you're asked for with Grep, Glob, Read and read-only shell commands (git log, ls, rg). Never edit files.
Answer with the facts only: file:line references, short code excerpts where they matter, and a one-line conclusion. Say plainly when something doesn't exist.`;

const WORKER_PROMPT = `You carry out a precisely specified code change. Do exactly what the instructions say, matching the surrounding code's style. Don't redesign or widen the scope; if the instructions are ambiguous or wrong, stop and report instead of guessing.
Finish with the files you changed and anything the caller must check.`;

/**
 * Subagents for a Claude Code session on `model`: only those on a cheaper model than the session,
 * so a Haiku session gets none.
 */
export function buildClaudeAgents(policy, model) {
  const session = claudeRank(model);
  const agents = {};
  const add = (name, tier, def) => {
    const choice = resolveTier(policy, "claude", tier);
    if (!choice?.model || claudeRank(choice.model) >= session) return;
    agents[name] = { ...def, model: choice.model, ...(choice.effort ? { effort: choice.effort } : {}) };
  };
  add("scout", "cheap", {
    description:
      "Cheap, fast read-only helper. Use it PROACTIVELY for any searching or reading: grep/glob, finding definitions and usages, reading and summarizing files, listing files, git history, running read-only commands. Returns file:line findings.",
    prompt: SCOUT_PROMPT,
    tools: ["Read", "Grep", "Glob", "Bash"],
  });
  add("worker", "standard", {
    description:
      "Mid-tier implementer for well-specified mechanical changes: renames, boilerplate, applying a described edit across files, writing tests for given behavior, fixing type or lint errors. Give it exact instructions; it makes no design decisions.",
    prompt: WORKER_PROMPT,
  });
  return agents;
}

/** System prompt that tells the main model what to hand to which subagent. */
export function delegationPrompt(agents) {
  const names = Object.keys(agents);
  if (!names.length) return "";
  const lines = ["Cost routing: you run on an expensive model. Delegate work that doesn't need you to cheaper subagents with the Agent tool:"];
  if (agents.scout) lines.push(`- scout (${agents.scout.model}): all searching, grepping, locating code and reading or summarizing files. Prefer it over running Grep/Glob/Read yourself when the answer takes more than one or two lookups.`);
  if (agents.worker) lines.push(`- worker (${agents.worker.model}): well-specified mechanical edits, once you've decided exactly what to change. Give it precise instructions.`);
  lines.push("Keep planning, design decisions, debugging unclear failures and the final review of changes yourself. Run independent subagent calls in parallel.");
  return lines.join("\n");
}

/** Whether a pane's model and effort are the router's to pick. */
export function shouldRoute(plan, policy) {
  const r = plan.route;
  return policy.routeSpawns && !plan.resume && !plan.plain && !!r && r.origin !== "user" && !r.explicitModel;
}

/** The task a routed pane gets, for classifying. */
export const taskOf = (plan) => text(plan.route?.task) || text(plan.prompt);

/**
 * Picks a pane's tier: the caller's hint, the rules, the LLM (`ask(task)` resolves to its reply)
 * and finally "standard". Resolves to `{ tier, via, reason }`.
 */
export async function pickTier(plan, policy, ask) {
  const hint = text(plan.route?.tier).toLowerCase();
  if (TIERS.includes(hint)) return { tier: hint, via: "hint", reason: "requested by the caller" };
  const task = taskOf(plan);
  const ruled = classifyRules(task, policy.rules);
  if (ruled) return { tier: ruled.tier, via: "rule", reason: `rule “${ruled.rule}”` };
  if (task && policy.llmFallback && ask) {
    try {
      const verdict = parseLlmVerdict(await ask(task));
      if (verdict) return { tier: verdict.tier, via: "llm", reason: verdict.reason };
    } catch {
      // No LLM available: fall through.
    }
  }
  return { tier: "standard", via: "default", reason: task ? "no rule matched" : "no task given" };
}

/** The spawn patch and log entry for a plan; `ask` as for `pickTier`. */
export async function routePlan(plan, policy, ask) {
  const patch = {};
  let entry = null;
  let model = plan.model;
  if (shouldRoute(plan, policy)) {
    const { tier, via, reason } = await pickTier(plan, policy, ask);
    const choice = resolveTier(policy, plan.provider, tier);
    if (choice) {
      if (choice.model) patch.model = model = choice.model;
      if (choice.effort) patch.effort = choice.effort;
    }
    entry = { paneId: plan.paneId, workspaceId: plan.workspaceId, provider: plan.provider, task: taskOf(plan).slice(0, 200), tier, via, reason, model: patch.model ?? plan.model, effort: patch.effort ?? plan.effort, from: plan.model };
  }
  if (policy.delegation && plan.provider === "claude" && !plan.plain) {
    const agents = buildClaudeAgents(policy, model);
    if (Object.keys(agents).length) {
      patch.agents = agents;
      patch.systemPrompt = delegationPrompt(agents);
    }
  }
  return { patch, entry };
}

// Workflows ------------------------------------------------------------------------------------

export const MAX_TASKS = 12;

/** Tidies a planner's task list: 1-based `n`, known tiers, dependencies only on earlier tasks. */
export function normalizeTasks(raw, offset = 0) {
  if (!Array.isArray(raw) || !raw.length) throw new Error("tasks must be a non-empty list");
  if (raw.length > MAX_TASKS) throw new Error(`at most ${MAX_TASKS} tasks; merge small ones`);
  return raw.map((t, i) => {
    const title = text(t?.title);
    if (!title) throw new Error(`task ${i + 1} needs a title`);
    const n = offset + i + 1;
    const tier = text(t.tier).toLowerCase();
    const deps = (Array.isArray(t.depends_on) ? t.depends_on : []).map(Number).filter((d) => Number.isInteger(d) && d >= 1 && d < n);
    return { n, title, details: text(t.details), tier: TIERS.includes(tier) ? tier : "", deps: [...new Set(deps)], status: "waiting", paneId: null, boardId: null, summary: "" };
  });
}

/** Tasks that can start now: waiting, their dependencies done, within the parallel limit. */
export function nextRunnable(wf) {
  if (wf.status !== "running") return [];
  const done = new Set(wf.tasks.filter((t) => t.status === "done").map((t) => t.n));
  const free = wf.maxParallel - wf.tasks.filter((t) => t.status === "running").length;
  if (free <= 0) return [];
  return wf.tasks.filter((t) => t.status === "waiting" && t.deps.every((d) => done.has(d))).slice(0, free);
}

/** Tasks that can never start: they depend on a failed one. */
export const blocked = (wf) => {
  const failed = new Set(wf.tasks.filter((t) => t.status === "failed").map((t) => t.n));
  let changed = true;
  while (changed) {
    changed = false;
    for (const t of wf.tasks) {
      if (!failed.has(t.n) && t.status === "waiting" && t.deps.some((d) => failed.has(d))) {
        failed.add(t.n);
        changed = true;
      }
    }
  }
  return wf.tasks.filter((t) => t.status === "waiting" && failed.has(t.n));
};

/** All tasks finished one way or another (nothing running or startable). */
export const settled = (wf) => wf.tasks.every((t) => t.status === "done" || t.status === "failed" || blocked(wf).includes(t));

export function plannerPrompt(wf) {
  return `You are the planner of winfer workflow ${wf.id}. Goal:

${wf.goal}

Look at the codebase as much as you need (hand searching to your scout subagent), then split the goal into 1–${MAX_TASKS} tasks for other agents. Each task runs in its own agent and git worktree, in parallel with the others unless it depends on them, so tasks must not edit the same files unless one depends on the other. Write each task's details as a complete brief: what to change, where, and what done looks like — the agent knows nothing else.

Give each task a tier: "cheap" (searching, small obvious edits), "standard" (well-specified changes, tests) or "frontier" (design, tricky debugging, cross-cutting work).

Then call workflow_plan with workflow "${wf.id}" and the tasks (depends_on lists earlier task numbers, starting at 1). Don't implement anything yourself; winfer starts the agents and a reviewer afterwards.`;
}

export function workerPrompt(wf, task) {
  const deps = task.deps.length ? `\nTasks it builds on (already done): ${task.deps.map((d) => `#${d}`).join(", ")}; their branches may need merging first if you depend on their code.` : "";
  return `You are an agent in winfer workflow ${wf.id} (goal: ${wf.goal.split("\n")[0].slice(0, 200)}).
Your task, #${task.n} of ${wf.tasks.length}: ${task.title}

${task.details || "(no further details)"}${deps}

Work only on this task, in this worktree. Commit your changes on the current branch when done. Then call workflow_done with workflow "${wf.id}", task ${task.n} and a short summary of what changed (or ok: false and why, if you couldn't do it).`;
}

export function reviewerPrompt(wf, branches) {
  const lines = wf.tasks.map((t) => `- #${t.n} ${t.title} [${t.status}]${branches[t.n] ? ` — branch ${branches[t.n]}` : ""}${t.summary ? `: ${t.summary}` : ""}`);
  return `You are the reviewer of winfer workflow ${wf.id}. Goal:

${wf.goal}

Agents worked on these tasks, each on its own branch:
${lines.join("\n")}

Review each branch's diff against the base branch: correctness, the goal actually met, conflicts between branches, tests (run them). Fix nothing large yourself. For each real problem call workflow_followup with workflow "${wf.id}", a title, a complete brief as details and a tier; winfer runs those and asks you to review again. When everything is right, call workflow_finish with workflow "${wf.id}" and a summary for the user, including the order to merge the branches in.`;
}

// Storage --------------------------------------------------------------------------------------

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) ?? "null") ?? {};
    return { policy: normalizePolicy(saved.policy), log: Array.isArray(saved.log) ? saved.log : [], delegations: saved.delegations ?? {}, workflows: Array.isArray(saved.workflows) ? saved.workflows : [] };
  } catch {
    return { policy: normalizePolicy(null), log: [], delegations: {}, workflows: [] };
  }
}

function createStore() {
  const data = load();
  const listeners = new Set();
  const save = () => {
    try {
      localStorage.setItem(STORAGE, JSON.stringify(data));
    } catch {
      // Storage full or blocked: routing still works for this run.
    }
    listeners.forEach((fn) => fn());
  };
  return {
    get policy() {
      return data.policy;
    },
    get log() {
      return data.log;
    },
    get delegations() {
      return data.delegations;
    },
    get workflows() {
      return data.workflows;
    },
    workflow(id) {
      const wf = data.workflows.find((w) => w.id === text(id));
      if (!wf) throw new Error(`No workflow “${id}”.`);
      return wf;
    },
    addWorkflow(wf) {
      data.workflows = [wf, ...data.workflows].slice(0, 20);
      save();
      return wf;
    },
    removeWorkflow(id) {
      data.workflows = data.workflows.filter((w) => w.id !== id);
      save();
    },
    /** Saves after changing a workflow in place. */
    touch: save,
    setPolicy(policy) {
      data.policy = normalizePolicy(policy);
      save();
    },
    addLog(entry) {
      data.log = [{ at: Date.now(), ...entry }, ...data.log].slice(0, MAX_LOG);
      save();
    },
    countDelegation(subagent) {
      data.delegations[subagent] = (data.delegations[subagent] ?? 0) + 1;
      save();
    },
    reset() {
      data.log = [];
      data.delegations = {};
      save();
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

// Plugin ---------------------------------------------------------------------------------------

export function activate(api) {
  const store = createStore();
  const verdicts = new Map();
  const ask = (task) => {
    if (!verdicts.has(task)) {
      const reply = api.llm({ system: LLM_SYSTEM, user: task.slice(0, 2000), provider: "claude", model: "haiku" }).catch(() => api.llm({ system: LLM_SYSTEM, user: task.slice(0, 2000) }));
      verdicts.set(task, reply);
      reply.catch(() => verdicts.delete(task));
    }
    return verdicts.get(task);
  };

  api.registerSpawnTransform(async (plan) => {
    const { patch, entry } = await routePlan(plan, store.policy, ask);
    if (entry) store.addLog(entry);
    return patch;
  });

  api.on("pane.tool", (e) => {
    if (e.subagent) store.countDelegation(e.subagent);
  });

  api.registerAgentTool({
    name: "route_task",
    description:
      "Ask winfer's model router which model tier a task needs (cheap, standard or frontier) and the model and effort that tier runs on, before handing the task to a new agent with spawn_agent. Doesn't start anything.",
    inputSchema: {
      type: "object",
      properties: {
        task: { type: "string", description: "The task, as you'd give it to the agent" },
        provider: { type: "string", description: "Agent to run it (default: yours)" },
      },
      required: ["task"],
    },
    async run(args, caller) {
      const task = text(args.task);
      if (!task) throw new Error("task is empty");
      const provider = text(args.provider) || caller.provider;
      const { tier, via, reason } = await pickTier({ route: { origin: "agent", task } }, store.policy, ask);
      const choice = resolveTier(store.policy, provider, tier);
      const runs = choice ? [choice.model ?? "default model", choice.effort ? `${choice.effort} effort` : null].filter(Boolean).join(", ") : "its default model";
      return `Tier: ${tier} (${via}: ${reason}). On ${provider} that runs ${runs}. Start it with spawn_agent { provider: "${provider}", tier: "${tier}", prompt: … }.`;
    },
  });

  const flow = createWorkflows(api, store);

  api.registerPage({ id: "router", title: "Router", icon: "network", mount: (el) => mountPage(el, api, store, flow) });

  api.registerTool({
    id: "workflow",
    title: "Start workflow",
    description: "Start a multi-agent workflow: a planner splits the goal into tasks, agents on cheaper models do them in parallel, and a reviewer checks the result",
    params: { goal: { kind: "span", slot: "instruction", ask: "What should the agents build?" } },
    palette: [{ title: "Workflow: start" }],
    async run(args) {
      const goal = text(args.goal);
      if (!goal) return api.openPage("router");
      try {
        const wf = await flow.start(goal, { workspaceId: api.state().activeWorkspaceId });
        api.toast(`Workflow ${wf.id} started: the planner is splitting it into tasks.`, "success");
      } catch (err) {
        api.toast(String(err?.message ?? err), "error");
      }
    },
  });

  api.registerTool({
    id: "open",
    title: "Model router",
    description: "Open the model router's settings and routing log",
    palette: [{ title: "Model router: open" }],
    run: () => api.openPage("router"),
  });
}

/** Runs workflows: spawns planner, workers and reviewer, and registers the tools they report with. */
function createWorkflows(api, store) {
  const paneOf = (paneId) => api.state().workspaces.flatMap((w) => w.panes).find((p) => p.id === paneId) ?? null;
  const nudged = new Set();

  const fail = (wf, task, why) => {
    task.status = "failed";
    task.summary = why;
    api.toast(`Workflow ${wf.id}: task #${task.n} failed — ${why}`, "error");
  };

  async function spawn(wf, opts) {
    try {
      return await api.spawnAgent({ provider: wf.provider, workspaceId: wf.workspaceId, ...opts });
    } catch (err) {
      if (wf.provider === "claude") throw err;
      // The workflow's agent went away; Claude Code is the fallback.
      return api.spawnAgent({ ...opts, provider: "claude", workspaceId: wf.workspaceId });
    }
  }

  async function board(tool, args, paneId) {
    try {
      return await api.callAgentTool(tool, args, paneId);
    } catch {
      return null; // Task Board off: the workflow runs without it.
    }
  }

  async function schedule(wf) {
    for (const task of blocked(wf)) fail(wf, task, "a task it depends on failed");
    for (const task of nextRunnable(wf)) {
      task.status = "running";
      try {
        task.paneId = await spawn(wf, {
          prompt: workerPrompt(wf, task),
          worktree: true,
          route: { origin: "workflow", tier: task.tier || undefined, task: `${task.title}\n${task.details}` },
        });
        if (task.boardId) await board("board_claim", { id: task.boardId }, task.paneId);
      } catch (err) {
        fail(wf, task, String(err?.message ?? err));
      }
    }
    if (wf.status === "running" && settled(wf)) await review(wf);
    store.touch();
  }

  async function review(wf) {
    wf.status = "reviewing";
    const branches = Object.fromEntries(wf.tasks.map((t) => [t.n, paneOf(t.paneId)?.worktree?.branch]).filter(([, b]) => b));
    try {
      wf.reviewerPane = await spawn(wf, { prompt: reviewerPrompt(wf, branches), worktree: false, route: { origin: "workflow", tier: "frontier", task: "review" } });
    } catch (err) {
      wf.status = "failed";
      api.toast(`Workflow ${wf.id}: couldn't start the reviewer — ${err?.message ?? err}`, "error");
    }
  }

  async function start(goal, { workspaceId, provider, maxParallel = 3 }) {
    const state = api.state();
    const ws = state.workspaces.find((w) => w.id === workspaceId) ?? state.workspaces.find((w) => w.id === state.activeWorkspaceId);
    if (!ws) throw new Error("No workspace to run the workflow in.");
    const id = `wf${Date.now().toString(36).slice(-5)}`;
    const wf = { id, goal, workspaceId: ws.id, provider: provider || "claude", maxParallel: Math.max(1, Math.min(Number(maxParallel) || 3, 6)), status: "planning", tasks: [], plannerPane: null, reviewerPane: null, summary: "", startedAt: Date.now() };
    store.addWorkflow(wf);
    try {
      wf.plannerPane = await spawn(wf, { prompt: plannerPrompt(wf), worktree: false, route: { origin: "workflow", tier: "frontier", task: goal } });
    } catch (err) {
      store.removeWorkflow(id);
      throw err;
    }
    store.touch();
    return wf;
  }

  async function addTasks(wf, raw, caller) {
    const tasks = normalizeTasks(raw, wf.tasks.length);
    for (const task of tasks) {
      const posted = await board("board_post", { title: `[${wf.id}] ${task.title}`, details: task.details }, caller.paneId);
      task.boardId = Number(posted?.match(/#(\d+)/)?.[1]) || null;
    }
    wf.tasks.push(...tasks);
    wf.status = "running";
    await schedule(wf);
    return tasks;
  }

  const wfArg = { type: "string", description: "Workflow id, e.g. wf1a2b3" };
  const taskSchema = {
    type: "object",
    properties: {
      title: { type: "string" },
      details: { type: "string", description: "Complete brief: what to change, where, what done looks like" },
      tier: { type: "string", enum: TIERS },
      depends_on: { type: "array", items: { type: "integer" }, description: "Numbers of earlier tasks (from 1) that must finish first" },
    },
    required: ["title", "details"],
  };

  api.registerAgentTool({
    name: "workflow_start",
    description:
      "Start a winfer multi-agent workflow for a bigger goal: a planner agent splits it into tasks, agents on the cheapest suitable models work on them in parallel in separate worktrees, and a reviewer checks the result. Returns the workflow id.",
    inputSchema: {
      type: "object",
      properties: {
        goal: { type: "string", description: "What to build or change, with any constraints" },
        provider: { type: "string", description: "Agent CLI for planner, workers and reviewer (default claude)" },
        max_parallel: { type: "integer", description: "Agents working at once (1–6, default 3)" },
      },
      required: ["goal"],
    },
    async run(args, caller) {
      const goal = text(args.goal);
      if (!goal) throw new Error("goal is empty");
      const wf = await start(goal, { workspaceId: caller.workspaceId, provider: text(args.provider), maxParallel: args.max_parallel });
      return `Started workflow ${wf.id}. A planner agent is splitting the goal into tasks; workflow_status shows progress.`;
    },
  });

  api.registerAgentTool({
    name: "workflow_plan",
    description: "Planner of a winfer workflow: hand over the task list. winfer then starts an agent per task (respecting depends_on and the parallel limit) and a reviewer when all are done.",
    inputSchema: { type: "object", properties: { workflow: wfArg, tasks: { type: "array", items: taskSchema } }, required: ["workflow", "tasks"] },
    async run(args, caller) {
      const wf = store.workflow(args.workflow);
      if (wf.status !== "planning") throw new Error(`Workflow ${wf.id} already has its tasks (status ${wf.status}).`);
      const tasks = await addTasks(wf, args.tasks, caller);
      const started = tasks.filter((t) => t.status === "running").map((t) => `#${t.n}`);
      return `Planned ${tasks.length} task${tasks.length === 1 ? "" : "s"}; started ${started.join(", ") || "none yet"}. You're done — winfer runs the rest.`;
    },
  });

  api.registerAgentTool({
    name: "workflow_done",
    description: "Agent in a winfer workflow: report your task finished (or failed, with ok: false). Commit your changes first.",
    inputSchema: {
      type: "object",
      properties: { workflow: wfArg, task: { type: "integer" }, summary: { type: "string", description: "What changed, or why it failed" }, ok: { type: "boolean", description: "false if you couldn't do it (default true)" } },
      required: ["workflow", "task", "summary"],
    },
    async run(args, caller) {
      const wf = store.workflow(args.workflow);
      const task = wf.tasks.find((t) => t.n === Number(args.task));
      if (!task) throw new Error(`Workflow ${wf.id} has no task #${args.task}.`);
      if (task.status === "done") return `Task #${task.n} was already done.`;
      const summary = text(args.summary);
      if (args.ok === false) fail(wf, task, summary || "the agent gave up");
      else {
        task.status = "done";
        task.summary = summary;
        if (task.boardId) await board("board_done", { id: task.boardId, summary }, caller.paneId);
      }
      await schedule(wf);
      return `Recorded task #${task.n} as ${task.status}. You can stop here.`;
    },
  });

  api.registerAgentTool({
    name: "workflow_followup",
    description: "Reviewer of a winfer workflow: add a follow-up task for a problem you found; winfer runs it and asks you to review again.",
    inputSchema: { type: "object", properties: { workflow: wfArg, ...taskSchema.properties }, required: ["workflow", "title", "details"] },
    async run(args, caller) {
      const wf = store.workflow(args.workflow);
      if (wf.status === "done") throw new Error(`Workflow ${wf.id} is finished.`);
      const [task] = await addTasks(wf, [{ title: args.title, details: args.details, tier: args.tier, depends_on: args.depends_on }], caller);
      return `Added follow-up #${task.n}${task.status === "running" ? " and started an agent on it" : ""}. A new review starts when it's done.`;
    },
  });

  api.registerAgentTool({
    name: "workflow_finish",
    description: "Reviewer of a winfer workflow: everything checks out — finish the workflow with a summary for the user.",
    inputSchema: { type: "object", properties: { workflow: wfArg, summary: { type: "string" } }, required: ["workflow", "summary"] },
    run(args) {
      const wf = store.workflow(args.workflow);
      wf.status = "done";
      wf.summary = text(args.summary);
      store.touch();
      api.toast(`Workflow ${wf.id} is done.`, "success");
      return "Workflow finished. Thanks!";
    },
  });

  api.registerAgentTool({
    name: "workflow_status",
    description: "Show a winfer workflow's tasks: who works on what, on which model, and what's done.",
    inputSchema: { type: "object", properties: { workflow: { ...wfArg, description: "Workflow id (default: the newest in your workspace)" } } },
    run(args, caller) {
      const wf = text(args.workflow) ? store.workflow(args.workflow) : store.workflows.find((w) => w.workspaceId === caller.workspaceId);
      if (!wf) return "No workflows in this workspace.";
      const lines = wf.tasks.map((t) => {
        const pane = paneOf(t.paneId);
        return `#${t.n} [${t.status}] ${t.title}${pane ? ` — ${pane.provider} ${pane.model ?? "default"}${pane.effort ? `/${pane.effort}` : ""}` : ""}${t.summary ? `: ${t.summary}` : ""}`;
      });
      return [`Workflow ${wf.id} (${wf.status}): ${wf.goal.split("\n")[0]}`, ...lines, wf.summary && `Summary: ${wf.summary}`].filter(Boolean).join("\n");
    },
  });

  // Agents that stop without reporting get one reminder; closed panes fail their task.
  api.on("pane.status", (e) => {
    if (e.status !== "done") return;
    for (const wf of store.workflows) {
      const task = wf.tasks.find((t) => t.paneId === e.paneId && t.status === "running");
      const role = task ? `task ${task.n}` : wf.plannerPane === e.paneId && wf.status === "planning" ? "plan" : wf.reviewerPane === e.paneId && wf.status === "reviewing" ? "review" : null;
      if (!role || nudged.has(`${wf.id}:${e.paneId}:${role}`)) continue;
      nudged.add(`${wf.id}:${e.paneId}:${role}`);
      const text =
        role === "plan"
          ? `Reminder: hand your task list to winfer with workflow_plan (workflow "${wf.id}").`
          : role === "review"
            ? `Reminder: finish the review with workflow_followup or workflow_finish (workflow "${wf.id}").`
            : `Reminder: if task #${task.n} is finished, commit and call workflow_done (workflow "${wf.id}", task ${task.n}); if you're stuck, call it with ok: false.`;
      api.execute({ type: "pane.sendText", target: { id: e.paneId }, text, submit: true });
    }
  });
  api.on("pane.closed", (e) => {
    for (const wf of store.workflows) {
      const task = wf.tasks.find((t) => t.paneId === e.paneId && t.status === "running");
      if (!task) continue;
      fail(wf, task, "its agent was closed");
      void schedule(wf);
    }
  });

  return {
    start,
    cancel(id) {
      const wf = store.workflow(id);
      wf.status = "cancelled";
      store.touch();
    },
    remove: (id) => store.removeWorkflow(id),
    paneOf,
  };
}

// Page -----------------------------------------------------------------------------------------

/** Builds an element: h("div", { class: "x", onclick }, child, "text", …). */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else if (key in el && typeof value !== "string") el[key] = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}

const CSS = `
.mr-page { max-width: 1040px; padding: 28px 32px 48px; gap: 20px; }
.mr-section { display: flex; flex-direction: column; gap: 10px; }
.mr-toggles { display: flex; flex-direction: column; gap: 10px; }
.mr-toggle { display: flex; gap: 10px; align-items: flex-start; font-size: 13px; color: var(--color-fg); }
.mr-toggle small { display: block; color: var(--color-fg-muted); font-size: 12px; margin-top: 2px; }
.mr-table { width: 100%; border-collapse: collapse; font-size: 12px; }
.mr-table th { text-align: left; font-weight: 600; color: var(--color-fg-muted); padding: 6px 8px; border-bottom: 1px solid var(--color-border); }
.mr-table td { padding: 6px 8px; border-bottom: 1px solid var(--color-border); vertical-align: middle; color: var(--color-fg); }
.mr-table .wf-input, .mr-table .wf-select { height: 28px; font-size: 12px; }
.mr-pair { display: flex; gap: 6px; }
.mr-pair .wf-input { width: 120px; }
.mr-pattern { width: 100%; font-family: var(--font-mono); }
.mr-tier { display: inline-block; padding: 1px 8px; border-radius: 999px; font-size: 11px; font-weight: 600; }
.mr-cheap { background: color-mix(in srgb, var(--color-success) 18%, transparent); color: var(--color-success); }
.mr-standard { background: var(--color-accent-soft); color: var(--color-accent); }
.mr-frontier { background: color-mix(in srgb, var(--color-danger, #d33) 16%, transparent); color: var(--color-danger, #d33); }
.mr-task { max-width: 380px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--color-fg-muted); }
.mr-stats { display: flex; gap: 18px; font-size: 12px; color: var(--color-fg-muted); flex-wrap: wrap; }
.mr-wf { display: flex; flex-direction: column; gap: 8px; padding: 10px 0; border-top: 1px solid var(--color-border); }
.mr-wf-goal { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; }
.mr-summary { font-size: 12px; line-height: 1.5; white-space: pre-wrap; color: var(--color-fg-muted); padding: 8px 10px; border-radius: 6px; background: color-mix(in srgb, var(--color-success) 8%, transparent); }
.mr-stats b { color: var(--color-fg); font-variant-numeric: tabular-nums; }
`;

function mountPage(root, api, store, flow) {
  const style = h("style", {}, CSS);
  const page = h("div", { class: "wf-page mr-page" });
  root.append(style, page);

  const update = (fn) => {
    const policy = clone(store.policy);
    fn(policy);
    store.setPolicy(policy);
  };

  function toggle(key, title, detail) {
    return h(
      "label",
      { class: "mr-toggle" },
      h("input", { type: "checkbox", class: "wf-switch", checked: store.policy[key], onchange: (e) => update((p) => (p[key] = e.target.checked)) }),
      h("span", {}, title, h("small", {}, detail)),
    );
  }

  function tierTable() {
    const providers = [...new Set(TIERS.flatMap((t) => Object.keys(store.policy.tiers[t])))];
    const cell = (tier, provider) => {
      const choice = store.policy.tiers[tier][provider] ?? { model: "", effort: "" };
      const efforts = EFFORTS[provider] ?? [];
      return h(
        "td",
        {},
        h(
          "div",
          { class: "mr-pair" },
          h("input", {
            class: "wf-input",
            value: choice.model,
            placeholder: "default model",
            onchange: (e) => update((p) => (p.tiers[tier][provider] = { ...choice, model: e.target.value.trim() })),
          }),
          h(
            "select",
            { class: "wf-select", onchange: (e) => update((p) => (p.tiers[tier][provider] = { ...choice, effort: e.target.value })) },
            h("option", { value: "", selected: !choice.effort }, "default effort"),
            efforts.map((level) => h("option", { value: level, selected: choice.effort === level }, level)),
          ),
        ),
      );
    };
    return h(
      "table",
      { class: "mr-table" },
      h("thead", {}, h("tr", {}, h("th", {}, "Tier"), providers.map((p) => h("th", {}, p)))),
      h("tbody", {}, TIERS.map((tier) => h("tr", {}, h("td", {}, h("span", { class: `mr-tier mr-${tier}` }, tier)), providers.map((p) => cell(tier, p))))),
    );
  }

  function rulesTable() {
    const rows = store.policy.rules.map((rule, i) =>
      h(
        "tr",
        {},
        h(
          "td",
          {},
          h(
            "select",
            { class: "wf-select", onchange: (e) => update((p) => (p.rules[i].tier = e.target.value)) },
            TIERS.map((t) => h("option", { value: t, selected: rule.tier === t }, t)),
          ),
        ),
        h(
          "td",
          { style: "width:100%" },
          h("input", {
            class: "wf-input mr-pattern",
            value: rule.pattern,
            title: rule.note,
            onchange: (e) => {
              if (!compile(e.target.value)) return api.toast("That isn't a valid regular expression.", "error");
              update((p) => (p.rules[i].pattern = e.target.value.trim()));
            },
          }),
        ),
        h("td", {}, h("button", { class: "wf-button wf-ghost", onclick: () => update((p) => p.rules.splice(i, 1)) }, "Remove")),
      ),
    );
    return h(
      "div",
      { class: "wf-stack" },
      h("table", { class: "mr-table" }, h("thead", {}, h("tr", {}, h("th", {}, "Tier"), h("th", {}, "Pattern (regex, first match wins)"), h("th", {}))), h("tbody", {}, rows)),
      h(
        "div",
        { class: "wf-row" },
        h("button", { class: "wf-button", onclick: () => update((p) => p.rules.push({ id: `r${Date.now()}`, tier: "cheap", pattern: "\\bexample\\b", note: "" })) }, "Add rule"),
        h("button", { class: "wf-button wf-ghost", onclick: () => update((p) => (p.rules = clone(DEFAULT_RULES))) }, "Restore defaults"),
      ),
    );
  }

  function logTable() {
    if (!store.log.length) return h("div", { class: "wf-empty" }, "Nothing routed yet. Agents that start other agents (spawn_agent) and workflows show up here.");
    return h(
      "table",
      { class: "mr-table" },
      h("thead", {}, h("tr", {}, ["When", "Agent", "Tier", "Runs on", "Why", "Task"].map((t) => h("th", {}, t)))),
      h(
        "tbody",
        {},
        store.log.slice(0, 50).map((e) =>
          h(
            "tr",
            {},
            h("td", {}, new Date(e.at).toLocaleTimeString()),
            h("td", {}, e.provider),
            h("td", {}, h("span", { class: `mr-tier mr-${e.tier}` }, e.tier)),
            h("td", { class: "wf-mono" }, [e.model ?? "default", e.effort].filter(Boolean).join(" · ")),
            h("td", {}, `${e.via}${e.reason ? `: ${e.reason}` : ""}`),
            h("td", { class: "mr-task", title: e.task }, e.task || "—"),
          ),
        ),
      ),
    );
  }

  // Kept across redraws, which happen whenever the log or a workflow changes.
  let draft = "";
  function workflows() {
    const goal = h("input", { class: "wf-input wf-grow", value: draft, placeholder: "Goal for a new workflow, e.g. add dark mode to the settings page", oninput: (e) => (draft = e.target.value) });
    const startIt = async () => {
      const g = goal.value.trim();
      if (!g) return goal.focus();
      try {
        draft = "";
        const wf = await flow.start(g, { workspaceId: api.state().activeWorkspaceId });
        api.toast(`Workflow ${wf.id} started.`, "success");
      } catch (err) {
        api.toast(String(err?.message ?? err), "error");
      }
    };
    goal.addEventListener("keydown", (e) => e.key === "Enter" && startIt());
    const card = (wf) => {
      const active = wf.status === "planning" || wf.status === "running" || wf.status === "reviewing";
      return h(
        "div",
        { class: "mr-wf" },
        h(
          "div",
          { class: "wf-row" },
          h("span", { class: "wf-badge" + (active ? " wf-accent" : "") }, wf.status),
          h("b", { class: "wf-grow mr-wf-goal", title: wf.goal }, wf.goal.split("\n")[0]),
          h("span", { class: "wf-subtle wf-mono" }, wf.id),
          active
            ? h("button", { class: "wf-button wf-ghost", onclick: () => flow.cancel(wf.id) }, "Stop")
            : h("button", { class: "wf-button wf-ghost", onclick: () => flow.remove(wf.id) }, "Remove"),
        ),
        wf.tasks.length
          ? h(
              "table",
              { class: "mr-table" },
              h(
                "tbody",
                {},
                wf.tasks.map((t) => {
                  const pane = flow.paneOf(t.paneId);
                  return h(
                    "tr",
                    {},
                    h("td", { class: "wf-mono" }, `#${t.n}`),
                    h("td", {}, h("span", { class: "wf-badge" }, t.status)),
                    h("td", { class: "mr-task", title: `${t.title}\n\n${t.details}${t.summary ? `\n\nResult: ${t.summary}` : ""}` }, t.title),
                    h("td", {}, t.tier ? h("span", { class: `mr-tier mr-${t.tier}` }, t.tier) : "auto"),
                    h("td", { class: "wf-mono" }, pane ? [pane.provider, pane.model ?? "default", pane.effort].filter(Boolean).join(" · ") : "—"),
                  );
                }),
              ),
            )
          : h("div", { class: "wf-subtle" }, wf.status === "planning" ? "The planner is splitting the goal into tasks…" : "No tasks."),
        wf.summary ? h("div", { class: "mr-summary" }, wf.summary) : null,
      );
    };
    return h(
      "div",
      { class: "wf-card mr-section" },
      h("h2", { class: "wf-h2" }, "Workflows"),
      h("div", { class: "wf-subtle" }, "A planner splits the goal into tasks, agents on the cheapest suitable models work on them in parallel worktrees, then a reviewer checks the branches."),
      h("div", { class: "wf-row" }, goal, h("button", { class: "wf-button wf-primary", onclick: startIt }, "Start")),
      store.workflows.map(card),
    );
  }

  function stats() {
    const byTier = Object.fromEntries(TIERS.map((t) => [t, store.log.filter((e) => e.tier === t).length]));
    const delegated = Object.entries(store.delegations).sort((a, b) => b[1] - a[1]);
    return h(
      "div",
      { class: "mr-stats" },
      TIERS.map((t) => h("span", {}, h("b", {}, byTier[t]), ` routed ${t}`)),
      delegated.length ? delegated.map(([name, n]) => h("span", {}, h("b", {}, n), ` handed to ${name}`)) : h("span", {}, "No subagent calls seen yet"),
    );
  }

  function render() {
    page.replaceChildren(
      h("div", { class: "wf-header" }, h("div", {}, h("h1", { class: "wf-h1" }, "Model router"), h("div", { class: "wf-subtle" }, "Cheaper models for work that doesn't need the best one. Applies to agents started after a change."))),
      stats(),
      h(
        "div",
        { class: "wf-card mr-section" },
        h("h2", { class: "wf-h2" }, "Routing"),
        h(
          "div",
          { class: "mr-toggles" },
          toggle("delegation", "Cheap subagents for Claude Code", "Sessions get a Haiku scout for searching and reading and a Sonnet worker for mechanical edits, and are told to use them. Only subagents cheaper than the session's model are added."),
          toggle("routeSpawns", "Route agents started by agents", "spawn_agent and workflow panes without an explicit model run on the tier their task needs. Panes you start keep your model."),
          toggle("llmFallback", "Ask an LLM when no rule matches", "A one-line Haiku call per new task (cached); otherwise unmatched work counts as standard."),
        ),
      ),
      workflows(),
      h("div", { class: "wf-card mr-section" }, h("h2", { class: "wf-h2" }, "Tiers"), h("div", { class: "wf-subtle" }, "Model and effort each tier runs on, per agent. Empty keeps the CLI's default."), tierTable()),
      h("div", { class: "wf-card mr-section" }, h("h2", { class: "wf-h2" }, "Rules"), rulesTable()),
      h(
        "div",
        { class: "wf-card mr-section" },
        h("div", { class: "wf-row" }, h("h2", { class: "wf-h2 wf-grow" }, "Routing log"), h("button", { class: "wf-button wf-ghost", onclick: () => store.reset() }, "Clear")),
        logTable(),
      ),
    );
  }

  render();
  const unsubscribe = store.subscribe(render);
  return () => {
    unsubscribe();
    style.remove();
  };
}
