// Telegram: messages you on Telegram when an agent needs input (or finishes), so you can approve,
// decline or answer it from your phone. Talks to the Telegram Bot API directly by long polling, so
// no server or public URL is needed. The bot only listens to the one account paired with it.

const STORAGE = "winfer.plugin.telegram";
const TOKEN_KEY = "BOT_TOKEN";
const API = "https://api.telegram.org";
const PAIR_TTL_MS = 10 * 60 * 1000;
/** Messages remembered for buttons and replies. */
const MAX_SENT = 200;
const MODES = {
  unfocused: "When winfer isn't the active window",
  always: "Always (I'm away)",
  paused: "Never (paused)",
};
/** How confirmations phrase each mode: "I'll message you …". */
const MODE_SAY = {
  unfocused: "only while winfer isn't the active window",
  always: "about everything, until you send /back",
  paused: "never, until you send /away or /back",
};
const STATUS_ICON = { idle: "⚪", working: "🔵", "needs-input": "🟠", done: "🟢", error: "🔴" };
const HELP = [
  "<b>winfer</b> messages you here when an agent needs you.",
  "",
  "• Tap <b>Approve</b> / <b>Decline</b> on permission requests.",
  "• <b>Reply</b> to a message to answer that agent or give it a new instruction.",
  "• /status lists your agents.",
  "• /away messages you about everything; /back only while winfer isn't in front; /pause stops messages.",
].join("\n");

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

const esc = (text) => String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** Resolves after `ms`, or as soon as `signal` aborts. */
const sleep = (ms, signal) =>
  new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => (clearTimeout(timer), resolve()), { once: true });
  });

function loadSettings() {
  const defaults = { chatId: null, userId: null, userName: "", mode: "unfocused", notifyDone: true, offset: 0 };
  try {
    return { ...defaults, ...JSON.parse(localStorage.getItem(STORAGE) ?? "{}") };
  } catch {
    return defaults;
  }
}

async function telegram(token, method, params = {}, signal) {
  const res = await fetch(`${API}/bot${token}/${method}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(params),
    signal,
  });
  const data = await res.json().catch(() => null);
  if (!data?.ok) {
    const err = new Error(data?.description ?? `Telegram answered ${res.status}`);
    err.code = data?.error_code ?? res.status;
    throw err;
  }
  return data.result;
}

export function activate(api) {
  const settings = loadSettings();
  const listeners = new Set();
  const changed = () => listeners.forEach((fn) => fn());
  const save = () => {
    localStorage.setItem(STORAGE, JSON.stringify(settings));
    changed();
  };

  /** Connection state shown on the page. */
  const conn = { token: null, bot: null, error: "" };
  /** The code the user sends with /start to pair, while pairing. */
  let pairing = null;
  /** Messages about panes, by Telegram message id; `bySeq` finds them from button data. */
  const sent = new Map();
  const bySeq = new Map();
  let nextSeq = 1;
  let poller = null;

  const call = (method, params, signal) => telegram(conn.token, method, params, signal);
  const paired = () => settings.chatId !== null && settings.userId !== null;
  const say = (text, extra = {}) => call("sendMessage", { chat_id: settings.chatId, text, parse_mode: "HTML", ...extra });

  const livePane = (paneId) => api.state().workspaces.flatMap((w) => w.panes).find((p) => p.id === paneId) ?? null;
  const title = (who) => `<b>${esc(who.label)}</b>${who.workspaceName ? ` · ${esc(who.workspaceName)}` : ""}`;

  function remember(entry) {
    sent.set(entry.messageId, entry);
    bySeq.set(entry.seq, entry);
    for (const [id, old] of sent) {
      if (sent.size <= MAX_SENT) break;
      sent.delete(id);
      bySeq.delete(old.seq);
    }
  }

  /** Marks a request as handled: removes its buttons and notes what happened. */
  async function close(entry, note) {
    if (!entry.open) return;
    entry.open = false;
    try {
      await call("editMessageText", { chat_id: settings.chatId, message_id: entry.messageId, text: `${entry.html}\n\n<i>${esc(note)}</i>`, parse_mode: "HTML" });
    } catch {
      // Deleted in Telegram, or too old to edit.
    }
  }

  const closeOpen = (paneId, note) => Promise.all([...sent.values()].filter((e) => e.open && e.paneId === paneId).map((e) => close(e, note)));

  const shouldMessage = () => paired() && conn.bot && settings.mode !== "paused" && (settings.mode === "always" || !document.hasFocus());

  async function sendRequest(paneId) {
    // The status hook fires as the prompt appears; give the terminal a moment to draw it.
    await sleep(600);
    const pane = livePane(paneId);
    const who = api.describePane(paneId);
    const request = await api.paneRequest(paneId);
    if (!pane || pane.status !== "needs-input" || !who || !request) return;
    await closeOpen(paneId, "No longer waiting");
    const approval = request.kind === "approval";
    const seq = nextSeq++;
    const html = `🟠 ${title(who)} ${approval ? "asks for permission" : "is waiting for you"}\n<pre>${esc(request.text)}</pre>`;
    const hint = approval ? "Reply to this message to decline with instructions." : "Reply to this message to answer.";
    const buttons = approval
      ? [{ text: "✅ Approve", callback_data: `a:${seq}` }, { text: "❌ Decline", callback_data: `d:${seq}` }]
      : [{ text: "⎋ Cancel", callback_data: `d:${seq}` }];
    const msg = await say(`${html}\n\n<i>${hint}</i>`, { reply_markup: { inline_keyboard: [buttons, [{ text: "🖥 Show screen", callback_data: `s:${seq}` }]] } });
    remember({ seq, messageId: msg.message_id, paneId, kind: request.kind, html, open: true });
  }

  async function sendFinished(paneId, status) {
    const who = api.describePane(paneId);
    if (!who) return;
    const seq = nextSeq++;
    const html = `${STATUS_ICON[status]} ${title(who)} ${status === "done" ? "finished" : "stopped with an error"}`;
    const msg = await say(`${html}\n\n<i>Reply to give it a new instruction.</i>`, {
      reply_markup: { inline_keyboard: [[{ text: "🖥 Show screen", callback_data: `s:${seq}` }]] },
    });
    remember({ seq, messageId: msg.message_id, paneId, kind: "finished", html, open: false });
  }

  async function showScreen(entry, replyTo) {
    const screen = (await api.readPane(entry.paneId, 40)) ?? "";
    const text = screen
      .split("\n")
      .map((l) => l.trimEnd())
      .join("\n")
      .replace(/\n{3,}/g, "\n\n")
      .slice(-3500)
      .trim();
    await say(`<pre>${esc(text || "(empty)")}</pre>`, { reply_parameters: { message_id: replyTo, allow_sending_without_reply: true } });
  }

  /** Sends what the user wrote to the pane `entry` is about. */
  async function answer(entry, text, messageId) {
    const reply = (t) => say(t, { reply_parameters: { message_id: messageId, allow_sending_without_reply: true } });
    const pane = livePane(entry.paneId);
    const who = api.describePane(entry.paneId);
    if (!pane || !who) return reply("That agent's pane is closed.");
    const target = { id: entry.paneId };
    if (entry.kind === "finished") {
      if (pane.status === "working") return reply(`${title(who)} is busy again; reply once it's done.`);
      if (pane.status === "needs-input") return reply(`${title(who)} is waiting on a request; answer that message instead.`);
    } else {
      if (!entry.open || pane.status !== "needs-input") return reply("That request was already handled.");
      // A permission prompt takes keys, not text: Esc declines it, then the text is the instruction.
      if (entry.kind === "approval") {
        api.execute({ type: "pane.interrupt", target });
        await sleep(300);
      }
      await close(entry, `💬 Answered: ${text.length > 200 ? `${text.slice(0, 199)}…` : text}`);
    }
    api.execute({ type: "pane.sendText", target, text, submit: true });
    api.toast(`Sent a Telegram reply to ${who.label}`);
    return reply(`✓ Sent to ${title(who)}.`);
  }

  async function onButton(query) {
    const ack = (text) => call("answerCallbackQuery", { callback_query_id: query.id, text });
    if (!paired() || query.message?.chat?.id !== settings.chatId || query.from?.id !== settings.userId) return ack("Not allowed.");
    const [action, seq] = String(query.data ?? "").split(":");
    const entry = bySeq.get(Number(seq));
    if (!entry) return ack("This message is from an earlier session.");
    if (action === "s") {
      await ack("");
      return showScreen(entry, query.message.message_id);
    }
    const pane = livePane(entry.paneId);
    const who = api.describePane(entry.paneId);
    if (!entry.open) return ack("Already handled.");
    if (!pane || !who || pane.status !== "needs-input") {
      await close(entry, pane ? "No longer waiting" : "Pane closed");
      return ack("The agent isn't waiting anymore.");
    }
    if (action === "a") {
      api.execute({ type: "pane.approve", target: { id: entry.paneId } });
      await close(entry, "✅ Approved");
      api.toast(`Approved ${who.label} from Telegram`);
      return ack("Approved");
    }
    if (action === "d") {
      api.execute({ type: "pane.interrupt", target: { id: entry.paneId } });
      await close(entry, entry.kind === "approval" ? "❌ Declined" : "⎋ Cancelled");
      api.toast(`Declined ${who.label} from Telegram`);
      return ack(entry.kind === "approval" ? "Declined" : "Cancelled");
    }
    return ack("");
  }

  function statusText() {
    const lines = [];
    for (const ws of api.state().workspaces) {
      const agents = ws.panes.filter((p) => p.provider !== "shell");
      if (!agents.length) continue;
      lines.push(`<b>${esc(ws.name)}</b>`);
      for (const pane of agents) {
        const who = api.describePane(pane.id);
        lines.push(`${STATUS_ICON[pane.status] ?? "⚪"} ${esc(who?.label ?? pane.provider)}${pane.agentTitle ? ` — ${esc(pane.agentTitle)}` : ""}`);
      }
    }
    return lines.length ? lines.join("\n") : "No agents are running.";
  }

  async function onCommand(command) {
    if (command === "status") return say(statusText());
    if (command === "away" || command === "back" || command === "pause") {
      settings.mode = { away: "always", back: "unfocused", pause: "paused" }[command];
      save();
      return say(`OK — I'll message you <b>${MODE_SAY[settings.mode]}</b>.`);
    }
    return say(HELP);
  }

  async function onMessage(msg) {
    const text = msg.text?.trim();
    if (!text || msg.chat?.type !== "private") return;
    const start = text.match(/^\/start(?:@\w+)?\s+(\S+)$/);
    if (start && pairing && Date.now() < pairing.expires && start[1] === pairing.code) {
      pairing = null;
      Object.assign(settings, { chatId: msg.chat.id, userId: msg.from.id, userName: msg.from.username ? `@${msg.from.username}` : (msg.from.first_name ?? "you") });
      save();
      api.toast(`Telegram paired with ${settings.userName}`, "success");
      return say(`✅ Paired with winfer.\n\n${HELP}`);
    }
    if (!paired() || msg.chat.id !== settings.chatId || msg.from?.id !== settings.userId) {
      // Strangers get nothing once paired; before that, a hint for the owner.
      if (!paired()) await telegram(conn.token, "sendMessage", { chat_id: msg.chat.id, text: "Open the Telegram page in winfer and press Pair to connect this chat." });
      return;
    }
    const command = text.match(/^\/(\w+)/);
    if (command) return onCommand(command[1].toLowerCase());
    const replyTo = msg.reply_to_message?.message_id;
    if (replyTo !== undefined) {
      const entry = sent.get(replyTo);
      if (entry) return answer(entry, text, msg.message_id);
      return say("I don't know that message anymore (winfer restarted?). Use /status, or reply to a newer message.");
    }
    // Not a reply: fine if exactly one agent is waiting.
    const waiting = [...sent.values()].filter((e) => e.open && livePane(e.paneId)?.status === "needs-input");
    if (waiting.length === 1) return answer(waiting[0], text, msg.message_id);
    return say(waiting.length ? `${waiting.length} agents are waiting — reply to the message of the one you mean.` : "Nothing is waiting for you. Reply to one of my messages to give that agent a new instruction.");
  }

  async function poll(signal) {
    while (!signal.aborted) {
      try {
        if (!conn.bot) {
          conn.bot = await call("getMe", {}, signal);
          conn.error = "";
          changed();
          await call("setMyCommands", {
            commands: [
              { command: "status", description: "List your agents" },
              { command: "away", description: "Message me about everything" },
              { command: "back", description: "Only message me while winfer isn't in front" },
              { command: "pause", description: "Stop messaging me" },
              { command: "help", description: "How this works" },
            ],
          }).catch(() => {});
        }
        const updates = await call("getUpdates", { offset: settings.offset, timeout: 50, allowed_updates: ["message", "callback_query"] }, signal);
        if (conn.error) {
          conn.error = "";
          changed();
        }
        for (const update of updates) {
          settings.offset = update.update_id + 1;
          try {
            if (update.callback_query) await onButton(update.callback_query);
            else if (update.message) await onMessage(update.message);
          } catch (err) {
            console.error("telegram: handling an update failed", err);
          }
        }
        if (updates.length) save();
      } catch (err) {
        if (signal.aborted) return;
        conn.error =
          err.code === 401 ? "Telegram rejected the bot token." : err.code === 409 ? "Something else is reading this bot's messages (is winfer open twice?)." : `Can't reach Telegram: ${err.message ?? err}`;
        changed();
        await sleep(err.code === 401 ? 60_000 : 5_000, signal);
      }
    }
  }

  async function connect() {
    poller?.abort();
    poller = null;
    Object.assign(conn, { token: null, bot: null, error: "" });
    const stored = (await api.vault.list()).some((s) => s.key === TOKEN_KEY);
    conn.token = stored ? await api.vault.reveal(TOKEN_KEY) : null;
    changed();
    if (!conn.token) return;
    poller = new AbortController();
    void poll(poller.signal);
  }

  const store = {
    settings,
    conn,
    pairing: () => pairing,
    subscribe(fn) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    async setToken(token) {
      const bot = await telegram(token, "getMe");
      await api.vault.set(TOKEN_KEY, token, { kind: "token", description: `Telegram bot @${bot.username}` });
      // Another bot: its chats and update ids don't carry over.
      Object.assign(settings, { chatId: null, userId: null, userName: "", offset: 0 });
      save();
      await connect();
    },
    async removeToken() {
      await api.vault.delete(TOKEN_KEY);
      Object.assign(settings, { chatId: null, userId: null, userName: "", offset: 0 });
      pairing = null;
      save();
      await connect();
    },
    startPairing() {
      const code = Array.from(crypto.getRandomValues(new Uint8Array(6)), (b) => (b % 36).toString(36)).join("");
      pairing = { code, expires: Date.now() + PAIR_TTL_MS };
      changed();
    },
    async unpair() {
      await say("winfer unpaired this chat; you won't get messages anymore.").catch(() => {});
      Object.assign(settings, { chatId: null, userId: null, userName: "" });
      save();
    },
    sendTest: () => say(`👋 Test from winfer. I'll message you <b>${MODE_SAY[settings.mode]}</b>.`),
    set(patch) {
      Object.assign(settings, patch);
      save();
    },
  };

  api.on("pane.status", (e) => {
    if (e.previous === "needs-input" && e.status !== "needs-input") void closeOpen(e.paneId, "Handled in winfer");
    if (e.provider === "shell" || !shouldMessage()) return;
    const send = e.status === "needs-input" ? sendRequest(e.paneId) : settings.notifyDone && (e.status === "done" || e.status === "error") ? sendFinished(e.paneId, e.status) : null;
    send?.catch((err) => console.error("telegram: sending failed", err));
  });

  api.registerPage({ id: "telegram", title: "Telegram", icon: "bot", mount: (el) => mountPage(el, store, api) });
  api.registerTool({
    id: "open",
    title: "Telegram",
    description: "Open the Telegram page to connect a bot that messages you when agents need input",
    palette: [{ title: "Telegram: settings" }],
    run: () => api.openPage("telegram"),
  });
  api.registerTool({
    id: "away",
    title: "Telegram: I'm away",
    description: "Send every agent request and finish to Telegram while I'm away from the computer",
    palette: true,
    run: () => {
      store.set({ mode: "always" });
      api.toast("Telegram: messaging you about everything");
    },
  });
  api.registerTool({
    id: "back",
    title: "Telegram: I'm back",
    description: "Only message me on Telegram while winfer isn't the active window",
    palette: true,
    run: () => {
      store.set({ mode: "unfocused" });
      api.toast("Telegram: only messaging you while winfer is in the background");
    },
  });

  void connect().catch((err) => {
    conn.error = String(err?.message ?? err);
    changed();
  });
  return () => poller?.abort();
}

function mountPage(root, store, api) {
  const { settings, conn } = store;
  let draft = "";
  let busy = false;
  let error = "";
  const page = h("div", { class: "wf-page" });
  root.append(page);

  async function act(work) {
    busy = true;
    error = "";
    render();
    try {
      await work();
    } catch (err) {
      error = String(err?.message ?? err);
    }
    busy = false;
    render();
  }

  function botCard() {
    if (!conn.token) {
      return h(
        "div",
        { class: "wf-card wf-stack" },
        h("div", { class: "wf-h2" }, "1. Create a bot"),
        h("div", { class: "wf-muted" }, "In Telegram, message @BotFather, send /newbot and follow the steps. Paste the token it gives you here."),
        h(
          "div",
          { class: "wf-inline" },
          h("input", {
            class: "wf-input wf-grow",
            type: "password",
            placeholder: "123456789:AA…",
            value: draft,
            oninput: (e) => (draft = e.target.value.trim()),
            onkeydown: (e) => e.key === "Enter" && draft && act(() => store.setToken(draft).then(() => (draft = ""))),
          }),
          h("button", { class: "wf-button wf-primary", disabled: busy, onclick: () => draft && act(() => store.setToken(draft).then(() => (draft = ""))) }, "Connect"),
        ),
      );
    }
    return h(
      "div",
      { class: "wf-card wf-row" },
      h(
        "div",
        { class: "wf-grow wf-stack", style: "gap:4px" },
        h("div", { class: "wf-h2" }, "Bot"),
        conn.bot ? h("div", {}, `Connected as @${conn.bot.username}`) : !conn.error && h("div", { class: "wf-muted" }, "Connecting…"),
        conn.error && h("div", { class: "wf-error" }, conn.error),
      ),
      h(
        "button",
        {
          class: "wf-button wf-danger",
          disabled: busy,
          onclick: async () => {
            if (await api.confirm("Disconnect the bot? You'll have to pair again.", { okLabel: "Disconnect" })) act(() => store.removeToken());
          },
        },
        "Disconnect",
      ),
    );
  }

  function pairCard() {
    if (!conn.bot) return null;
    if (settings.chatId !== null) {
      return h(
        "div",
        { class: "wf-card wf-row" },
        h("div", { class: "wf-grow wf-stack", style: "gap:4px" }, h("div", { class: "wf-h2" }, "Account"), h("div", {}, `Paired with ${settings.userName}`)),
        h("button", { class: "wf-button", disabled: busy, onclick: () => act(() => store.sendTest()) }, "Send test message"),
        h("button", { class: "wf-button wf-ghost", disabled: busy, onclick: () => act(() => store.unpair()) }, "Unpair"),
      );
    }
    const pairing = store.pairing();
    const link = pairing && `https://t.me/${conn.bot.username}?start=${pairing.code}`;
    return h(
      "div",
      { class: "wf-card wf-stack" },
      h("div", { class: "wf-h2" }, "2. Pair your account"),
      pairing
        ? [
            h("div", {}, "In Telegram, send this to ", h("b", {}, `@${conn.bot.username}`), " (valid for 10 minutes):"),
            h("div", { class: "wf-mono", style: "font-size:16px;user-select:text" }, `/start ${pairing.code}`),
            h("div", { class: "wf-subtle", style: "user-select:text;word-break:break-all" }, `Or open ${link}`),
          ]
        : [
            h("div", { class: "wf-muted" }, "Only the paired account can approve or answer agents."),
            h("div", {}, h("button", { class: "wf-button wf-primary", onclick: () => store.startPairing() }, "Pair")),
          ],
    );
  }

  function whenCard() {
    if (settings.chatId === null) return null;
    return h(
      "div",
      { class: "wf-card wf-stack" },
      h("div", { class: "wf-h2" }, "When to message you"),
      h(
        "select",
        { class: "wf-select", onchange: (e) => store.set({ mode: e.target.value }) },
        Object.entries(MODES).map(([value, label]) => h("option", { value, selected: settings.mode === value }, label)),
      ),
      h(
        "label",
        { class: "wf-inline" },
        h("input", { type: "checkbox", class: "wf-switch", checked: settings.notifyDone, onchange: (e) => store.set({ notifyDone: e.target.checked }) }),
        "Also when an agent finishes or fails",
      ),
      h("div", { class: "wf-subtle" }, "Send /away from Telegram (or ⌘K → “Telegram: I'm away”) when you leave the computer, and /back when you return."),
    );
  }

  function render() {
    page.replaceChildren(
      h(
        "div",
        { class: "wf-header" },
        h("div", { class: "wf-h1" }, "Telegram"),
        h("div", { class: "wf-muted" }, "Approve, decline or answer your agents from your phone."),
      ),
      h("div", { class: "wf-stack" }, botCard(), pairCard(), whenCard(), error && h("div", { class: "wf-error" }, error)),
    );
  }

  render();
  const unsubscribe = store.subscribe(render);
  return () => {
    unsubscribe();
    page.remove();
  };
}
