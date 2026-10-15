// Task Board: one board per workspace where agents post, claim and finish tasks and leave notes
// for each other. Agents use it through agent tools; the user sees it as a page and can add tasks,
// hand one to a new agent pane, or send the board to every agent.

const STORAGE = "winfer.plugin.task-board";
const MAX_NOTES = 50;
const COLUMNS = [
  ["todo", "To do"],
  ["claimed", "In progress"],
  ["done", "Done"],
];

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

/** Boards by workspace id, saved in the webview's localStorage. */
function createStore() {
  let boards = {};
  try {
    boards = JSON.parse(localStorage.getItem(STORAGE) ?? "{}") ?? {};
  } catch {
    boards = {};
  }
  const listeners = new Set();

  const board = (workspaceId) => (boards[workspaceId] ??= { nextId: 1, tasks: [], notes: [] });

  function save() {
    localStorage.setItem(STORAGE, JSON.stringify(boards));
    listeners.forEach((fn) => fn());
  }

  function task(workspaceId, id) {
    const found = board(workspaceId).tasks.find((t) => t.id === Number(id));
    if (!found) throw new Error(`No task #${id} on this workspace's board. Use board_list to see the tasks.`);
    return found;
  }

  return {
    board,
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    /** Tells subscribers to redraw, e.g. when panes changed but the boards didn't. */
    notify: () => listeners.forEach((fn) => fn()),
    add(workspaceId, { title, details = "", by }) {
      const b = board(workspaceId);
      const now = Date.now();
      const t = { id: b.nextId++, title, details, status: "todo", owner: null, by, summary: "", createdAt: now, updatedAt: now };
      b.tasks.push(t);
      save();
      return t;
    },
    claim(workspaceId, id, owner) {
      const t = task(workspaceId, id);
      if (t.status === "done") throw new Error(`Task #${t.id} is already done.`);
      if (t.status === "claimed" && t.owner?.paneId !== owner.paneId) throw new Error(`Task #${t.id} is already claimed by ${t.owner.label}.`);
      Object.assign(t, { status: "claimed", owner, updatedAt: Date.now() });
      save();
      return t;
    },
    finish(workspaceId, id, summary, owner) {
      const t = task(workspaceId, id);
      Object.assign(t, { status: "done", summary, owner: owner ?? t.owner, updatedAt: Date.now() });
      save();
      return t;
    },
    release(workspaceId, id) {
      const t = task(workspaceId, id);
      Object.assign(t, { status: "todo", owner: null, summary: "", updatedAt: Date.now() });
      save();
      return t;
    },
    remove(workspaceId, id) {
      const b = board(workspaceId);
      b.tasks = b.tasks.filter((t) => t.id !== Number(id));
      save();
    },
    clearDone(workspaceId) {
      const b = board(workspaceId);
      b.tasks = b.tasks.filter((t) => t.status !== "done");
      save();
    },
    /** Puts every task `paneId` claimed back in To do; returns them. */
    releasePane(workspaceId, paneId) {
      const released = board(workspaceId).tasks.filter((t) => t.status === "claimed" && t.owner?.paneId === paneId);
      for (const t of released) Object.assign(t, { status: "todo", owner: null, updatedAt: Date.now() });
      if (released.length) save();
      return released;
    },
    note(workspaceId, text, by) {
      const b = board(workspaceId);
      b.notes = [...b.notes, { text, by, at: Date.now() }].slice(-MAX_NOTES);
      save();
    },
    clearNotes(workspaceId) {
      board(workspaceId).notes = [];
      save();
    },
  };
}

const describeTask = (t) =>
  [
    `#${t.id} [${t.status === "claimed" ? `in progress: ${t.owner?.label ?? "someone"}` : t.status === "done" ? "done" : "to do"}] ${t.title}`,
    t.details && `    ${t.details.replace(/\n/g, "\n    ")}`,
    t.summary && `    Result: ${t.summary.replace(/\n/g, "\n    ")}`,
  ]
    .filter(Boolean)
    .join("\n");

function describeBoard(b, { notes = true } = {}) {
  const open = b.tasks.filter((t) => t.status !== "done");
  const done = b.tasks.filter((t) => t.status === "done");
  const parts = [open.length ? `Open tasks:\n${open.map(describeTask).join("\n")}` : "No open tasks."];
  if (done.length) parts.push(`Done:\n${done.map(describeTask).join("\n")}`);
  if (notes && b.notes.length) parts.push(`Notes (oldest first):\n${b.notes.map((n) => `- ${n.by}: ${n.text}`).join("\n")}`);
  return parts.join("\n\n");
}

const taskPrompt = (t) =>
  `Task #${t.id} from the winfer task board: ${t.title}${t.details ? `\n\n${t.details}` : ""}\n\n` +
  `It is claimed for you. Check board_list for notes from other agents first. When finished, call board_done with id ${t.id} and a short summary.`;

const text = (value) => (typeof value === "string" ? value.trim() : "");

export function activate(api) {
  const store = createStore();
  /** Task ids waiting for the agent pane spawned for them, by workspace id, oldest first. */
  const pendingAssign = new Map();

  const activeWorkspace = () => api.state().activeWorkspaceId;
  const workspaceName = (id) => api.state().workspaces.find((w) => w.id === id)?.name ?? "this workspace";

  /** Opens a new agent pane in the task's workspace and gives it the task. */
  function assign(workspaceId, id) {
    const t = store.board(workspaceId).tasks.find((x) => x.id === Number(id));
    if (!t || t.status !== "todo") throw new Error(t ? `Task #${t.id} isn't in To do.` : `No task #${id}.`);
    if (activeWorkspace() !== workspaceId) api.execute({ type: "workspace.switch", target: { id: workspaceId } });
    pendingAssign.set(workspaceId, [...(pendingAssign.get(workspaceId) ?? []), t.id]);
    api.execute({ type: "pane.spawn", provider: "default", prompt: taskPrompt(t) });
    api.execute({ type: "ui.view", view: "workspace" });
  }

  function broadcast(workspaceId) {
    const b = store.board(workspaceId);
    if (b.tasks.length === 0 && b.notes.length === 0) return api.toast("The task board is empty.");
    if (activeWorkspace() !== workspaceId) api.execute({ type: "workspace.switch", target: { id: workspaceId } });
    api.execute({
      type: "pane.broadcast",
      text: `Here is the winfer task board for this workspace. Claim a task with board_claim before working on it.\n\n${describeBoard(b)}`,
    });
    api.toast("Sent the board to every agent", "success");
  }

  // Agent tools ----------------------------------------------------------------------------------

  api.registerAgentTool({
    name: "board_list",
    description:
      "Show the task board shared by all agents in your winfer workspace: open tasks (and who is on them), finished tasks with their results, and notes other agents left. Check it before starting work.",
    inputSchema: { type: "object", properties: {} },
    run: (_args, caller) => describeBoard(store.board(caller.workspaceId)),
  });

  api.registerAgentTool({
    name: "board_post",
    description:
      "Add a task to the shared task board so another agent (or you, later) can pick it up. Use it to split work or record follow-ups. Set claim to true to take it yourself.",
    inputSchema: {
      type: "object",
      properties: {
        title: { type: "string", description: "Short, actionable title" },
        details: { type: "string", description: "What done looks like, files involved, constraints" },
        claim: { type: "boolean", description: "Claim the new task for yourself (default false)" },
      },
      required: ["title"],
    },
    run(args, caller) {
      const title = text(args.title);
      if (!title) throw new Error("title is empty");
      let t = store.add(caller.workspaceId, { title, details: text(args.details), by: caller.label });
      if (args.claim === true) t = store.claim(caller.workspaceId, t.id, { paneId: caller.paneId, label: caller.label });
      return `Posted task #${t.id}${args.claim === true ? " and claimed it for you" : ""}.`;
    },
  });

  api.registerAgentTool({
    name: "board_claim",
    description: "Claim a task on the shared task board before you work on it, so other agents leave it alone.",
    inputSchema: { type: "object", properties: { id: { type: "integer", description: "Task number" } }, required: ["id"] },
    run(args, caller) {
      const t = store.claim(caller.workspaceId, args.id, { paneId: caller.paneId, label: caller.label });
      return `You have task #${t.id}: ${t.title}${t.details ? `\n\n${t.details}` : ""}\n\nCall board_done when finished, or board_release to give it back.`;
    },
  });

  api.registerAgentTool({
    name: "board_done",
    description: "Mark a task on the shared task board as done, with a short summary of what changed so other agents and the user can build on it.",
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "integer", description: "Task number" },
        summary: { type: "string", description: "What you did and anything others need to know" },
      },
      required: ["id", "summary"],
    },
    run(args, caller) {
      const before = store.board(caller.workspaceId).tasks.find((t) => t.id === Number(args.id));
      if (before?.status === "claimed" && before.owner?.paneId !== caller.paneId)
        throw new Error(`Task #${before.id} is claimed by ${before.owner.label}; only they can finish it.`);
      const t = store.finish(caller.workspaceId, args.id, text(args.summary), { paneId: caller.paneId, label: caller.label });
      api.toast(`${caller.label} finished #${t.id}: ${t.title}`, "success");
      const open = store.board(caller.workspaceId).tasks.filter((x) => x.status === "todo");
      return `Task #${t.id} is done.${open.length ? ` ${open.length} task${open.length === 1 ? "" : "s"} still waiting in To do (board_list).` : ""}`;
    },
  });

  api.registerAgentTool({
    name: "board_release",
    description: "Give a task you claimed on the shared task board back to To do, e.g. when you're blocked or it belongs to someone else.",
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "integer", description: "Task number" },
        reason: { type: "string", description: "Why, left as a note for the next agent" },
      },
      required: ["id"],
    },
    run(args, caller) {
      const t = store.board(caller.workspaceId).tasks.find((x) => x.id === Number(args.id));
      if (t?.status === "claimed" && t.owner?.paneId !== caller.paneId) throw new Error(`Task #${t.id} is claimed by ${t.owner.label}, not you.`);
      store.release(caller.workspaceId, args.id);
      const reason = text(args.reason);
      if (reason) store.note(caller.workspaceId, `Released #${args.id}: ${reason}`, caller.label);
      return `Task #${args.id} is back in To do.`;
    },
  });

  api.registerAgentTool({
    name: "board_note",
    description:
      "Leave a short note on the shared task board for the other agents in your workspace, e.g. \"changed the API response shape in src/api.ts\" or \"don't touch the migrations, I'm on them\".",
    inputSchema: { type: "object", properties: { text: { type: "string", description: "The note" } }, required: ["text"] },
    run(args, caller) {
      const note = text(args.text);
      if (!note) throw new Error("text is empty");
      store.note(caller.workspaceId, note, caller.label);
      return "Note added.";
    },
  });

  // Events ---------------------------------------------------------------------------------------

  api.on("pane.opened", (e) => {
    store.notify();
    if (e.provider === "shell") return;
    const queue = pendingAssign.get(e.workspaceId);
    const id = queue?.shift();
    if (id === undefined) return;
    const pane = api.state().workspaces.flatMap((w) => w.panes).find((p) => p.id === e.paneId);
    try {
      store.claim(e.workspaceId, id, { paneId: e.paneId, label: pane?.customTitle || pane?.agentTitle || `new ${e.provider} pane` });
    } catch {
      // Someone else took it in the meantime; the new agent will see that on the board.
    }
  });

  api.on("pane.closed", (e) => {
    store.notify();
    const released = store.releasePane(e.workspaceId, e.paneId);
    if (released.length) api.toast(`Pane closed: ${released.map((t) => `#${t.id}`).join(", ")} back in To do`);
  });

  /** "pane:task" pairs already warned about, so an agent that stops after every turn warns once. */
  const warned = new Set();
  api.on("pane.status", (e) => {
    store.notify();
    if (e.status !== "done" && e.status !== "error") return;
    const mine = store
      .board(e.workspaceId)
      .tasks.filter((t) => t.status === "claimed" && t.owner?.paneId === e.paneId && !warned.has(`${e.paneId}:${t.id}`));
    if (mine.length === 0) return;
    mine.forEach((t) => warned.add(`${e.paneId}:${t.id}`));
    const titles = mine.map((t) => `#${t.id} ${t.title}`).join(", ");
    if (e.status === "error") api.toast(`${e.provider} hit an error while on ${titles}`, "error");
    else api.toast(`${e.provider} stopped without finishing ${titles}`);
  });

  // Page, palette and voice ----------------------------------------------------------------------

  api.registerPage({ id: "board", title: "Tasks", icon: "square-kanban", mount: (el) => mountPage(el, api, store, { assign, broadcast }) });

  api.registerTool({
    id: "open",
    title: "Task board",
    description: "Open the task board that shows what each agent is working on",
    palette: [{ title: "Task board: open" }],
    run: () => api.openPage("board"),
  });

  api.registerTool({
    id: "add",
    title: "Add task",
    description: "Add a task to the shared task board for the agents to pick up",
    params: { title: { kind: "span", slot: "instruction", ask: "What's the task?" } },
    palette: [{ title: "Task board: add task" }],
    run(args) {
      const title = text(args.title);
      if (!title) return api.openPage("board");
      const t = store.add(activeWorkspace(), { title, by: "you" });
      api.toast(`Added #${t.id}: ${t.title}`, "success");
    },
  });

  api.registerTool({
    id: "assign_next",
    title: "Hand next task to a new agent",
    description: "Start a new agent pane and give it the next open task from the task board",
    palette: [{ title: "Task board: hand next task to a new agent" }],
    run() {
      const workspaceId = activeWorkspace();
      const next = store.board(workspaceId).tasks.find((t) => t.status === "todo");
      if (!next) return api.toast(`No open tasks in ${workspaceName(workspaceId)}.`);
      assign(workspaceId, next.id);
    },
  });

  api.registerTool({
    id: "broadcast",
    title: "Send task board to agents",
    description: "Send the task board with all open tasks and notes to every agent in this workspace",
    risk: "confirm",
    palette: [{ title: "Task board: send to all agents" }],
    run: () => broadcast(activeWorkspace()),
  });
}

const CSS = `
.tb-page { max-width: 1240px; padding: 28px 32px 48px; gap: 18px; container-type: inline-size; }
.tb-title { display: flex; align-items: center; gap: 10px; }
.tb-stats { display: flex; gap: 14px; margin-top: 6px; font-size: 12px; color: var(--color-fg-muted); }
.tb-stats b { color: var(--color-fg); font-weight: 600; font-variant-numeric: tabular-nums; }
.tb-icon { width: 14px; height: 14px; flex-shrink: 0; }
.tb-composer { display: flex; flex-direction: column; border: 1px solid var(--color-border-strong); border-radius: 10px; background: var(--color-panel); transition: border-color .15s, box-shadow .15s; }
.tb-composer:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.tb-composer-row { display: flex; align-items: center; gap: 8px; padding: 6px 6px 6px 12px; }
.tb-composer-row > .tb-icon { color: var(--color-fg-subtle); }
.tb-composer input, .tb-composer textarea { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; color: var(--color-fg); font: inherit; font-size: 13px; user-select: text; }
.tb-composer input { height: 28px; }
.tb-composer textarea { resize: none; margin: 0 12px 10px 34px; padding: 8px 0 0; border-top: 1px solid var(--color-border); font-size: 12px; line-height: 1.5; color: var(--color-fg-muted); min-height: 52px; }
.tb-composer:not(.tb-open) textarea { display: none; }
.tb-hint { font-size: 11px; color: var(--color-fg-subtle); white-space: nowrap; }
.tb-kbd { font-family: var(--font-mono); font-size: 10px; padding: 1px 5px; border-radius: 4px; border: 1px solid var(--color-border-strong); }
.tb-body { display: grid; grid-template-columns: minmax(0, 1fr) 280px; gap: 16px; align-items: start; }
.tb-lanes { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; align-items: start; }
@container (max-width: 980px) { .tb-body { grid-template-columns: minmax(0, 1fr); } }
@container (max-width: 640px) { .tb-lanes { grid-template-columns: minmax(0, 1fr); } }
.tb-lane { display: flex; flex-direction: column; gap: 8px; padding: 10px; border-radius: 12px; background: color-mix(in srgb, var(--color-panel-raised) 70%, transparent); border: 1px solid var(--color-border); min-height: 180px; }
.tb-lane-head { display: flex; align-items: center; gap: 8px; height: 26px; padding: 0 0 2px 4px; font-size: 12px; font-weight: 600; color: var(--color-fg); }
.tb-lane-head .tb-count { font-weight: 500; color: var(--color-fg-subtle); font-variant-numeric: tabular-nums; }
.tb-lane-head .tb-spacer { flex: 1; }
.tb-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; background: var(--tb-tone); }
.tb-todo { --tb-tone: var(--color-fg-subtle); }
.tb-claimed { --tb-tone: var(--color-accent); }
.tb-done { --tb-tone: var(--color-success); }
.tb-card { position: relative; display: flex; flex-direction: column; gap: 8px; padding: 10px 12px 10px 14px; border-radius: 9px; background: var(--color-panel); border: 1px solid var(--color-border); overflow: hidden; transition: border-color .15s, transform .15s; }
.tb-card::before { content: ""; position: absolute; left: 0; top: 0; bottom: 0; width: 3px; background: var(--tb-tone); opacity: .8; }
.tb-card:hover { border-color: var(--color-border-strong); }
.tb-card-top { display: flex; gap: 8px; align-items: baseline; }
.tb-id { font-family: var(--font-mono); font-size: 11px; color: var(--color-fg-subtle); }
.tb-card-title { flex: 1; min-width: 0; font-size: 13px; font-weight: 500; line-height: 1.4; color: var(--color-fg); overflow-wrap: anywhere; }
.tb-done .tb-card-title { color: var(--color-fg-muted); }
.tb-details { font-size: 12px; line-height: 1.5; color: var(--color-fg-muted); white-space: pre-wrap; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden; }
.tb-summary { font-size: 12px; line-height: 1.5; color: var(--color-fg-muted); white-space: pre-wrap; overflow-wrap: anywhere; padding: 6px 8px; border-radius: 6px; background: color-mix(in srgb, var(--color-success) 8%, transparent); }
.tb-meta { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 11px; color: var(--color-fg-subtle); }
.tb-owner { display: inline-flex; align-items: center; gap: 6px; max-width: 100%; padding: 2px 8px 2px 6px; border-radius: 999px; background: var(--color-active); color: var(--color-fg); font-size: 11px; cursor: pointer; border: 0; font-family: inherit; }
.tb-owner:hover { background: var(--color-hover); }
.tb-owner span:last-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tb-status { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; background: var(--color-fg-subtle); }
.tb-status.tb-working { background: var(--color-accent); animation: tb-pulse 1.4s ease-in-out infinite; }
.tb-status.tb-needs-input { background: var(--color-warning); }
.tb-status.tb-error { background: var(--color-danger); }
.tb-status.tb-done { background: var(--color-success); }
@keyframes tb-pulse { 50% { opacity: .35; } }
.tb-actions { display: flex; gap: 4px; flex-wrap: wrap; margin: 0 -4px -2px; opacity: .6; transition: opacity .12s; }
.tb-card:hover .tb-actions, .tb-card:focus-within .tb-actions { opacity: 1; }
.tb-act { display: inline-flex; align-items: center; gap: 5px; height: 24px; padding: 0 8px; border-radius: 6px; border: 0; background: transparent; color: var(--color-fg-muted); font: inherit; font-size: 11px; cursor: pointer; }
.tb-act:hover { background: var(--color-hover); color: var(--color-fg); }
.tb-act.tb-main { background: var(--color-accent-soft); color: var(--color-accent); }
.tb-act.tb-main:hover { background: var(--color-accent); color: var(--color-on-accent); }
.tb-act.tb-del:hover { color: var(--color-danger); }
.tb-act .tb-icon { width: 12px; height: 12px; }
.tb-empty { flex: 1; display: flex; align-items: center; justify-content: center; text-align: center; padding: 22px 12px; border: 1px dashed var(--color-border-strong); border-radius: 9px; font-size: 12px; color: var(--color-fg-subtle); line-height: 1.5; }
.tb-feed { display: flex; flex-direction: column; border-radius: 12px; border: 1px solid var(--color-border); background: color-mix(in srgb, var(--color-panel-raised) 70%, transparent); }
.tb-feed-head { display: flex; align-items: center; gap: 8px; height: 26px; padding: 10px 10px 8px 14px; font-size: 12px; font-weight: 600; color: var(--color-fg); }
.tb-feed-head .tb-spacer { flex: 1; }
.tb-feed-list { display: flex; flex-direction: column; max-height: 560px; overflow-y: auto; padding: 0 6px 6px; }
.tb-note { display: flex; gap: 10px; padding: 8px; border-radius: 8px; }
.tb-note:hover { background: var(--color-hover); }
.tb-avatar { width: 24px; height: 24px; letter-spacing: -.02em; border-radius: 6px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-size: 10px; font-weight: 700; color: white; background: var(--tb-avatar); }
.tb-note-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.tb-note-by { display: flex; gap: 6px; align-items: baseline; font-size: 11px; color: var(--color-fg-muted); }
.tb-note-by b { font-weight: 600; color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tb-note-text { font-size: 12px; line-height: 1.5; color: var(--color-fg); white-space: pre-wrap; overflow-wrap: anywhere; user-select: text; }
.tb-feed .tb-empty { margin: 0 8px 8px; flex: none; }
.tb-tools { margin-top: 2px; font-size: 11px; color: var(--color-fg-subtle); }
.tb-tools code { font-family: var(--font-mono); color: var(--color-fg-muted); }
`;

const ICONS = {
  plus: "M12 5v14M5 12h14",
  send: "M22 2 11 13M22 2l-7 20-4-9-9-4 20-7z",
  play: "M6 4l14 8-14 8z",
  check: "M20 6 9 17l-5-5",
  undo: "M3 7v6h6M3 13a9 9 0 1 0 3-7.7L3 8",
  trash: "M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  sweep: "M3 6h18M7 12h10M10 18h4",
  board: "M3 3h18v18H3zM9 3v18M15 3v18",
};

function icon(name) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("class", "tb-icon");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", ICONS[name]);
  svg.append(path);
  return svg;
}

function ago(time) {
  const s = Math.max(0, Math.round((Date.now() - time) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}

const AVATAR_COLORS = ["#d97757", "#6d7cff", "#25a28f", "#c26bd6", "#e0a33a", "#4a9fe0"];
function avatarColor(name) {
  let hash = 0;
  for (const c of name) hash = (hash * 31 + c.charCodeAt(0)) | 0;
  return AVATAR_COLORS[Math.abs(hash) % AVATAR_COLORS.length];
}
/** "pane 2 (Claude)" → "Claude", "you" → "You". */
const shortName = (label) => (/\(([^)]+)\)\s*$/.exec(label)?.[1] ?? label).replace(/^./, (c) => c.toUpperCase());

function mountPage(root, api, store, actions) {
  const style = h("style", {}, CSS);
  const page = h("div", { class: "wf-page tb-page" });
  root.append(style, page);
  let workspaceId = api.state().activeWorkspaceId;

  const act = (work) => {
    try {
      work();
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
  };

  const findPane = (paneId) => api.state().workspaces.flatMap((w) => w.panes).find((p) => p.id === paneId);

  const workspaceSelect = h("select", {
    class: "wf-select",
    onchange: (e) => {
      workspaceId = e.target.value;
      render();
    },
  });
  const stats = h("div", { class: "tb-stats" });

  // Composer: Enter adds, Shift+Enter or "Details" opens a field for more.
  const title = h("input", { placeholder: "Add a task for the agents, e.g. “Write tests for the login form”" });
  const details = h("textarea", { rows: 2, placeholder: "Details: what done looks like, files involved, constraints" });
  const composer = h("form", { class: "tb-composer" });
  const toggleDetails = h(
    "button",
    {
      type: "button",
      class: "tb-act",
      onclick: () => {
        composer.classList.toggle("tb-open");
        (composer.classList.contains("tb-open") ? details : title).focus();
      },
    },
    "Details",
  );
  const submit = () => {
    if (!title.value.trim()) return title.focus();
    act(() => store.add(workspaceId, { title: title.value.trim(), details: details.value.trim(), by: "you" }));
    title.value = "";
    details.value = "";
    composer.classList.remove("tb-open");
    title.focus();
  };
  title.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      composer.classList.add("tb-open");
      details.focus();
    }
  });
  details.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      submit();
    }
  });
  composer.addEventListener("submit", (e) => {
    e.preventDefault();
    submit();
  });
  composer.append(
    h(
      "div",
      { class: "tb-composer-row" },
      icon("plus"),
      title,
      h("span", { class: "tb-hint" }, h("span", { class: "tb-kbd" }, "⇧↩"), " details"),
      toggleDetails,
      h("button", { class: "wf-button wf-primary", type: "submit" }, "Add task"),
    ),
    details,
  );

  const lanes = h("div", { class: "tb-lanes" });
  const feed = h("div", { class: "tb-feed" });

  page.append(
    h(
      "div",
      { class: "wf-header" },
      h(
        "div",
        {},
        h("div", { class: "tb-title" }, h("h1", { class: "wf-h1" }, "Task board")),
        stats,
        h(
          "div",
          { class: "tb-tools" },
          "Agents use ",
          h("code", {}, "board_list"),
          " · ",
          h("code", {}, "board_claim"),
          " · ",
          h("code", {}, "board_done"),
          " · ",
          h("code", {}, "board_note"),
        ),
      ),
      h(
        "div",
        { class: "wf-inline" },
        workspaceSelect,
        h(
          "button",
          { class: "wf-button", title: "Type the board into every agent pane of this workspace", onclick: () => act(() => actions.broadcast(workspaceId)) },
          h("span", { class: "wf-inline", style: "gap:6px" }, icon("send"), "Send to agents"),
        ),
      ),
    ),
    composer,
    h("div", { class: "tb-body" }, lanes, feed),
  );

  const button = (label, iconName, onclick, cls = "", tip) =>
    h("button", { class: `tb-act ${cls}`, title: tip, onclick: () => act(onclick) }, icon(iconName), label);

  function owner(t) {
    const pane = findPane(t.owner.paneId);
    const status = t.status === "done" ? "done" : (pane?.status ?? "closed");
    const label = pane || t.status === "done" ? t.owner.label : `${t.owner.label} · closed`;
    if (!pane) return h("span", { class: "tb-owner", style: "cursor:default" }, h("span", { class: `tb-status tb-${status}` }), h("span", {}, label));
    return h(
      "button",
      {
        class: "tb-owner",
        title: `Show this pane (${pane.status})`,
        onclick: () => {
          api.execute({ type: "pane.focus", target: { id: pane.id } });
          api.execute({ type: "ui.view", view: "workspace" });
        },
      },
      h("span", { class: `tb-status tb-${status}` }),
      h("span", {}, label),
    );
  }

  function card(t) {
    const buttons = [];
    if (t.status === "todo") buttons.push(button("Assign", "play", () => actions.assign(workspaceId, t.id), "tb-main", "Start a new agent pane on this task"));
    if (t.status === "claimed") {
      buttons.push(button("Done", "check", () => store.finish(workspaceId, t.id, "Marked done by you."), "tb-main"));
      buttons.push(button("Release", "undo", () => store.release(workspaceId, t.id), "", "Put back in To do"));
    }
    if (t.status === "done") buttons.push(button("Reopen", "undo", () => store.release(workspaceId, t.id)));
    buttons.push(
      h(
        "button",
        {
          class: "tb-act tb-del",
          title: "Delete task",
          onclick: async () => {
            if (await api.confirm(`Delete task #${t.id} “${t.title}”?`, { title: "Delete task", okLabel: "Delete" })) act(() => store.remove(workspaceId, t.id));
          },
        },
        icon("trash"),
      ),
    );
    return h(
      "div",
      { class: `tb-card tb-${t.status}` },
      h("div", { class: "tb-card-top" }, h("span", { class: "tb-card-title" }, t.title), h("span", { class: "tb-id" }, `#${t.id}`)),
      t.details && h("div", { class: "tb-details", title: t.details }, t.details),
      t.summary && h("div", { class: "tb-summary" }, t.summary),
      h("div", { class: "tb-meta" }, t.owner && owner(t), h("span", {}, `from ${t.by === "you" ? "you" : shortName(t.by)} · ${ago(t.updatedAt)}`)),
      h("div", { class: "tb-actions" }, buttons),
    );
  }

  const EMPTY = {
    todo: "Add a task above, or let agents post their own with board_post.",
    claimed: "Nobody is on a task. Assign one to start an agent.",
    done: "Finished tasks show up here with the agent's summary.",
  };

  function render() {
    const { workspaces } = api.state();
    if (!workspaces.some((w) => w.id === workspaceId)) workspaceId = workspaces[0]?.id ?? workspaceId;
    workspaceSelect.replaceChildren(...workspaces.map((w) => h("option", { value: w.id, selected: w.id === workspaceId }, w.name)));
    workspaceSelect.style.display = workspaces.length > 1 ? "" : "none";

    const b = store.board(workspaceId);
    const count = (status) => b.tasks.filter((t) => t.status === status).length;
    stats.replaceChildren(
      ...COLUMNS.map(([status, label]) => h("span", { class: `tb-${status}`, style: "display:inline-flex;align-items:center;gap:6px" }, h("span", { class: "tb-dot" }), h("b", {}, String(count(status))), label.toLowerCase())),
    );

    lanes.replaceChildren(
      ...COLUMNS.map(([status, label]) => {
        const tasks = b.tasks.filter((t) => t.status === status);
        if (status === "done") tasks.sort((x, y) => y.updatedAt - x.updatedAt);
        return h(
          "div",
          { class: `tb-lane tb-${status}` },
          h(
            "div",
            { class: "tb-lane-head" },
            h("span", { class: "tb-dot" }),
            label,
            h("span", { class: "tb-count" }, String(tasks.length)),
            h("span", { class: "tb-spacer" }),
            status === "done" && tasks.length > 0 && button("Clear", "sweep", () => store.clearDone(workspaceId), "", "Remove finished tasks"),
          ),
          tasks.length ? tasks.map(card) : h("div", { class: "tb-empty" }, EMPTY[status]),
        );
      }),
    );

    feed.replaceChildren(
      h(
        "div",
        { class: "tb-feed-head" },
        "Notes",
        h("span", { class: "tb-count", style: "font-weight:500;color:var(--color-fg-subtle)" }, String(b.notes.length)),
        h("span", { class: "tb-spacer" }),
        b.notes.length > 0 && button("Clear", "sweep", () => store.clearNotes(workspaceId)),
      ),
      b.notes.length
        ? h(
            "div",
            { class: "tb-feed-list" },
            [...b.notes].reverse().map((n) => {
              const name = shortName(n.by);
              return h(
                "div",
                { class: "tb-note" },
                h("span", { class: "tb-avatar", style: `--tb-avatar:${avatarColor(name)}` }, name.slice(0, 2)),
                h(
                  "div",
                  { class: "tb-note-body" },
                  h("div", { class: "tb-note-by" }, h("b", { title: n.by }, n.by), h("span", { title: new Date(n.at).toLocaleString() }, ago(n.at))),
                  h("div", { class: "tb-note-text" }, n.text),
                ),
              );
            }),
          )
        : h("div", { class: "tb-empty" }, "Agents leave each other notes here with board_note: changed APIs, files to avoid, blockers."),
    );
  }

  const unsubscribe = store.subscribe(render);
  // Keeps "5m ago" current.
  const timer = setInterval(render, 60_000);
  render();
  title.focus();
  return () => {
    unsubscribe();
    clearInterval(timer);
  };
}
