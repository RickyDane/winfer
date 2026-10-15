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
      store.claim(e.workspaceId, id, { paneId: e.paneId, label: pane?.agentTitle || `new ${e.provider} pane` });
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

  api.registerPage({ id: "board", title: "Tasks", icon: "star", mount: (el) => mountPage(el, api, store, { assign, broadcast }) });

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

function mountPage(root, api, store, actions) {
  const page = h("div", { class: "wf-page" });
  root.append(page);
  const state = api.state();
  let workspaceId = state.activeWorkspaceId;

  const act = (work) => {
    try {
      work();
    } catch (err) {
      api.toast(String(err?.message ?? err), "error");
    }
  };

  const paneExists = (paneId) => api.state().workspaces.some((w) => w.panes.some((p) => p.id === paneId));

  const workspaceSelect = h("select", {
    class: "wf-select",
    onchange: (e) => {
      workspaceId = e.target.value;
      render();
    },
  });
  const title = h("input", { class: "wf-input wf-grow", placeholder: "New task, e.g. “Add tests for the login form”" });
  const details = h("textarea", { class: "wf-input", rows: 2, placeholder: "Details (optional): what done looks like, files, constraints" });
  const addForm = h(
    "form",
    {
      class: "wf-card wf-stack",
      style: "gap:8px",
      onsubmit: (e) => {
        e.preventDefault();
        if (!title.value.trim()) return title.focus();
        store.add(workspaceId, { title: title.value.trim(), details: details.value.trim(), by: "you" });
        title.value = "";
        details.value = "";
        title.focus();
      },
    },
    h("div", { class: "wf-inline", style: "flex-wrap:nowrap" }, title, h("button", { class: "wf-button wf-primary", type: "submit" }, "Add task")),
    details,
  );
  const columns = h("div", { style: "display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px;align-items:start" });
  const notes = h("div", { class: "wf-card wf-stack", style: "gap:6px" });

  page.append(
    h(
      "div",
      { class: "wf-header" },
      h(
        "div",
        { class: "wf-stack", style: "gap:4px" },
        h("h1", { class: "wf-h1" }, "Task board"),
        h(
          "p",
          { class: "wf-muted", style: "margin:0" },
          "Shared by the agents in a workspace. They read it with ",
          h("span", { class: "wf-mono" }, "board_list"),
          ", take work with ",
          h("span", { class: "wf-mono" }, "board_claim"),
          ", report with ",
          h("span", { class: "wf-mono" }, "board_done"),
          " and leave each other notes.",
        ),
      ),
      h(
        "div",
        { class: "wf-inline" },
        workspaceSelect,
        h("button", { class: "wf-button", onclick: () => act(() => actions.broadcast(workspaceId)) }, "Send to agents"),
      ),
    ),
    addForm,
    columns,
    notes,
  );

  function card(t) {
    const live = t.owner && paneExists(t.owner.paneId);
    const buttons = [];
    if (t.status === "todo")
      buttons.push(h("button", { class: "wf-button wf-primary", title: "Start a new agent pane on this task", onclick: () => act(() => actions.assign(workspaceId, t.id)) }, "Assign"));
    if (live)
      buttons.push(
        h(
          "button",
          {
            class: "wf-button wf-ghost",
            onclick: () => {
              api.execute({ type: "pane.focus", target: { id: t.owner.paneId } });
              api.execute({ type: "ui.view", view: "workspace" });
            },
          },
          "Show pane",
        ),
      );
    if (t.status === "claimed") {
      buttons.push(h("button", { class: "wf-button", onclick: () => act(() => store.release(workspaceId, t.id)) }, "Release"));
      buttons.push(h("button", { class: "wf-button", onclick: () => act(() => store.finish(workspaceId, t.id, "Marked done by you.")) }, "Done"));
    }
    if (t.status === "done") buttons.push(h("button", { class: "wf-button", onclick: () => act(() => store.release(workspaceId, t.id)) }, "Reopen"));
    buttons.push(
      h(
        "button",
        {
          class: "wf-button wf-danger wf-ghost",
          onclick: async () => {
            if (await api.confirm(`Delete task #${t.id} “${t.title}”?`, { title: "Delete task", okLabel: "Delete" })) act(() => store.remove(workspaceId, t.id));
          },
        },
        "Delete",
      ),
    );
    return h(
      "div",
      { class: "wf-card wf-stack", style: "gap:6px;padding:10px" },
      h("div", { class: "wf-inline", style: "flex-wrap:nowrap;align-items:baseline" }, h("span", { class: "wf-subtle wf-mono" }, `#${t.id}`), h("strong", { style: "font-size:13px" }, t.title)),
      t.details && h("div", { class: "wf-muted", style: "white-space:pre-wrap" }, t.details),
      t.owner && h("div", {}, h("span", { class: `wf-badge${t.status === "claimed" ? " wf-accent" : ""}` }, live || t.status === "done" ? t.owner.label : `${t.owner.label} (closed)`)),
      t.summary && h("div", { class: "wf-subtle", style: "white-space:pre-wrap" }, t.summary),
      h("div", { class: "wf-subtle" }, `Added by ${t.by}`),
      h("div", { class: "wf-inline" }, buttons),
    );
  }

  function render() {
    const { workspaces } = api.state();
    if (!workspaces.some((w) => w.id === workspaceId)) workspaceId = workspaces[0]?.id ?? workspaceId;
    workspaceSelect.replaceChildren(...workspaces.map((w) => h("option", { value: w.id, selected: w.id === workspaceId }, w.name)));
    workspaceSelect.style.display = workspaces.length > 1 ? "" : "none";

    const b = store.board(workspaceId);
    columns.replaceChildren(
      ...COLUMNS.map(([status, label]) => {
        const tasks = b.tasks.filter((t) => t.status === status);
        if (status === "done") tasks.reverse();
        return h(
          "div",
          { class: "wf-stack", style: "gap:8px;min-width:0" },
          h(
            "div",
            { class: "wf-inline", style: "justify-content:space-between" },
            h("h2", { class: "wf-h2" }, label, " ", h("span", { class: "wf-badge" }, String(tasks.length))),
            status === "done" && tasks.length > 0 && h("button", { class: "wf-button wf-ghost", onclick: () => act(() => store.clearDone(workspaceId)) }, "Clear"),
          ),
          tasks.length ? tasks.map(card) : h("div", { class: "wf-empty" }, status === "todo" ? "Nothing waiting." : status === "claimed" ? "No agent is on a task." : "Nothing finished yet."),
        );
      }),
    );

    notes.replaceChildren(
      h(
        "div",
        { class: "wf-inline", style: "justify-content:space-between" },
        h("h2", { class: "wf-h2" }, "Notes between agents"),
        b.notes.length > 0 && h("button", { class: "wf-button wf-ghost", onclick: () => act(() => store.clearNotes(workspaceId)) }, "Clear"),
      ),
      b.notes.length
        ? [...b.notes].reverse().map((n) =>
            h(
              "div",
              { class: "wf-row" },
              h("div", { class: "wf-grow", style: "white-space:pre-wrap" }, n.text),
              h("span", { class: "wf-subtle", title: new Date(n.at).toLocaleString() }, n.by),
            ),
          )
        : h("div", { class: "wf-empty" }, "Agents leave notes here with board_note."),
    );
  }

  const unsubscribe = store.subscribe(render);
  render();
  title.focus();
  return unsubscribe;
}
