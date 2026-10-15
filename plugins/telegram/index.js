// Telegram: messages you on Telegram when an agent needs input (or finishes), so you can approve,
// decline or answer it from your phone. Talks to the Telegram Bot API directly by long polling, so
// no server or public URL is needed. The bot only listens to the one account paired with it.

const STORAGE = "winfer.plugin.telegram";
const TOKEN_KEY = "BOT_TOKEN";
const API = "https://api.telegram.org";
const PAIR_TTL_MS = 10 * 60 * 1000;
/** Messages remembered for buttons and replies. */
const MAX_SENT = 200;
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

/** Longest agent reply quoted in a message; Telegram caps a message at 4096 characters. */
const MAX_REPLY = 3300;

/** An agent's Markdown reply as Telegram HTML: code blocks, inline code, bold and headings. */
function replyHtml(markdown) {
  let text = markdown.trim().replace(/\n{3,}/g, "\n\n");
  if (text.length > MAX_REPLY) text = `${text.slice(0, MAX_REPLY).trimEnd()}…`;
  // Splitting on fences keeps them balanced even when the cut lands inside a block.
  return text
    .split(/```[^\n]*\n?/)
    .map((part, i) =>
      i % 2
        ? `<pre>${esc(part.replace(/\n$/, ""))}</pre>`
        : part
            .split(/`([^`\n]+)`/)
            .map((bit, j) =>
              j % 2
                ? `<code>${esc(bit)}</code>`
                : esc(bit)
                    .replace(/\*\*([^*\n]+)\*\*/g, "<b>$1</b>")
                    .replace(/^#{1,6} +(.+)$/gm, "<b>$1</b>"),
            )
            .join(""),
    )
    .join("");
}

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
    // `lastReply` arrived in winfer 0.1.35; older apps send the message without the reply.
    const reply = status === "done" ? (api.lastReply?.(paneId) ?? null) : null;
    const extra = { reply_markup: { inline_keyboard: [[{ text: "🖥 Show screen", callback_data: `s:${seq}` }]] } };
    const send = (body) => say(`${body}\n\n<i>Reply to give it a new instruction.</i>`, extra);
    // Telegram refuses the whole message if it can't parse the converted reply; send it without.
    const msg = reply ? await send(`${html}\n\n${replyHtml(reply)}`).catch(() => send(html)) : await send(html);
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
        lines.push(`${STATUS_ICON[pane.status] ?? "⚪"} ${esc(who?.label ?? pane.provider)}${pane.customTitle || pane.agentTitle ? ` — ${esc(pane.customTitle || pane.agentTitle)}` : ""}`);
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

  api.registerPage({ id: "telegram", title: "Telegram", icon: "send", mount: (el) => mountPage(el, store, api) });
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


const CSS = `
.tg-page { max-width: 760px; gap: 16px; container-type: inline-size; }
.tg-icon { width: 14px; height: 14px; flex-shrink: 0; }
.tg-hero { display: flex; align-items: center; gap: 14px; }
.tg-hero-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.tg-logo { width: 40px; height: 40px; border-radius: 11px; flex-shrink: 0; display: flex; align-items: center; justify-content: center; color: white; background: linear-gradient(160deg, #3fb5ec, #1f8fd0); box-shadow: inset 0 1px 0 rgb(255 255 255 / .2), 0 4px 14px rgb(31 143 208 / .25); }
.tg-logo svg { width: 22px; height: 22px; margin-left: -2px; }
.tg-logo.tg-small { width: 30px; height: 30px; border-radius: 8px; box-shadow: none; }
.tg-logo.tg-small svg { width: 16px; height: 16px; }
.tg-pill { display: inline-flex; align-items: center; gap: 7px; height: 24px; padding: 0 10px; border-radius: 999px; font-size: 11px; font-weight: 500; white-space: nowrap; color: var(--color-fg-muted); background: var(--color-panel-raised); border: 1px solid var(--color-border); }
.tg-dot { width: 7px; height: 7px; border-radius: 50%; flex-shrink: 0; background: var(--tg-tone, var(--color-fg-subtle)); }
.tg-ok { --tg-tone: var(--color-success); }
.tg-wait { --tg-tone: var(--color-warning); }
.tg-bad { --tg-tone: var(--color-danger); }
.tg-wait .tg-dot { animation: tg-pulse 1.4s ease-in-out infinite; }
.tg-ok .tg-dot { box-shadow: 0 0 0 3px color-mix(in srgb, var(--color-success) 20%, transparent); }
@keyframes tg-pulse { 50% { opacity: .35; } }
.tg-alert { display: flex; gap: 8px; align-items: flex-start; padding: 9px 12px; border-radius: 8px; font-size: 12px; line-height: 1.5; color: var(--color-danger); background: color-mix(in srgb, var(--color-danger) 9%, transparent); border: 1px solid color-mix(in srgb, var(--color-danger) 25%, transparent); }
.tg-alert .tg-icon { margin-top: 2px; }
.tg-section { display: flex; flex-direction: column; gap: 8px; }
.tg-label { font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: .05em; color: var(--color-fg-subtle); padding-left: 2px; }

.tg-steps { padding: 6px 0; }
.tg-step { position: relative; display: flex; gap: 14px; padding: 14px 18px; }
.tg-step:not(:last-child)::after { content: ""; position: absolute; left: 31px; top: 44px; bottom: -10px; width: 1px; background: var(--color-border-strong); }
.tg-num { position: relative; z-index: 1; width: 26px; height: 26px; border-radius: 50%; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-size: 12px; font-weight: 600; font-variant-numeric: tabular-nums; color: var(--color-fg-subtle); background: var(--color-panel); border: 1px solid var(--color-border-strong); }
.tg-step.tg-active .tg-num { color: var(--color-on-accent); background: var(--color-accent); border-color: var(--color-accent); box-shadow: 0 0 0 4px var(--color-accent-soft); }
.tg-step.tg-complete .tg-num { color: var(--color-success); background: color-mix(in srgb, var(--color-success) 12%, var(--color-panel)); border-color: color-mix(in srgb, var(--color-success) 40%, transparent); }
.tg-step-body { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 10px; padding-top: 3px; }
.tg-step-head { display: flex; align-items: center; gap: 10px; min-height: 20px; }
.tg-step-title { flex: 1; font-size: 13px; font-weight: 600; color: var(--color-fg); }
.tg-step:not(.tg-active):not(.tg-complete) .tg-step-title { color: var(--color-fg-subtle); font-weight: 500; }
.tg-step-done { font-size: 12px; color: var(--color-fg-muted); }
.tg-list { margin: 0; padding: 0; list-style: none; display: flex; flex-direction: column; gap: 6px; counter-reset: tg; }
.tg-list li { counter-increment: tg; display: flex; gap: 10px; font-size: 12px; line-height: 1.6; color: var(--color-fg-muted); }
.tg-list li::before { content: counter(tg, lower-alpha); width: 16px; flex-shrink: 0; color: var(--color-fg-subtle); font-family: var(--font-mono); font-size: 11px; line-height: 19px; }
.tg-list b { color: var(--color-fg); font-weight: 500; }
.tg-code { font-family: var(--font-mono); font-size: 11px; padding: 1px 6px; border-radius: 4px; background: var(--color-active); color: var(--color-fg); user-select: text; }

.tg-field { display: flex; align-items: center; gap: 6px; padding: 4px 4px 4px 10px; border-radius: 8px; border: 1px solid var(--color-border-strong); background: var(--color-panel); transition: border-color .15s, box-shadow .15s; }
.tg-field:focus-within { border-color: var(--color-accent); box-shadow: 0 0 0 3px var(--color-accent-soft); }
.tg-field > .tg-icon { color: var(--color-fg-subtle); }
.tg-field input { flex: 1; min-width: 0; height: 26px; border: 0; outline: none; background: transparent; color: var(--color-fg); font-family: var(--font-mono); font-size: 12px; user-select: text; }
.tg-field input::placeholder { color: var(--color-fg-subtle); }
.tg-icon-btn { display: inline-flex; align-items: center; justify-content: center; width: 26px; height: 26px; flex-shrink: 0; border-radius: 6px; border: 0; background: transparent; color: var(--color-fg-subtle); }
.tg-icon-btn:hover { background: var(--color-hover); color: var(--color-fg); }
.tg-hint { display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--color-fg-subtle); }
.tg-hint .tg-icon { width: 12px; height: 12px; }

.tg-pair { display: flex; flex-direction: column; gap: 10px; padding: 14px; border-radius: 10px; background: var(--color-panel); border: 1px solid var(--color-border); }
.tg-pair-top { display: flex; align-items: center; justify-content: space-between; gap: 10px; font-size: 12px; color: var(--color-fg-muted); }
.tg-pair-top b { color: var(--color-fg); font-weight: 500; }
.tg-command { display: flex; align-items: center; gap: 10px; padding: 8px 8px 8px 14px; border-radius: 8px; background: var(--color-panel-raised); border: 1px dashed var(--color-border-strong); }
.tg-command code { flex: 1; min-width: 0; font-family: var(--font-mono); font-size: 18px; font-weight: 500; letter-spacing: .02em; color: var(--color-fg); user-select: text; }
.tg-command code span { color: var(--color-accent); }
.tg-link { display: flex; align-items: center; gap: 8px; font-size: 11px; color: var(--color-fg-subtle); }
.tg-link code { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); color: var(--color-fg-muted); user-select: text; }
.tg-expiry { font-variant-numeric: tabular-nums; }
.tg-expired { color: var(--color-warning); }

.tg-who { display: flex; align-items: center; gap: 12px; padding: 12px 14px; }
.tg-who + .tg-who { border-top: 1px solid var(--color-border); }
.tg-who-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.tg-who-name { font-size: 13px; font-weight: 500; color: var(--color-fg); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tg-who-sub { display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--color-fg-muted); }
.tg-who-sub .tg-dot { width: 6px; height: 6px; }
.tg-avatar { width: 30px; height: 30px; border-radius: 50%; flex-shrink: 0; display: flex; align-items: center; justify-content: center; font-size: 11px; font-weight: 700; color: white; background: linear-gradient(160deg, #f59e6b, #d9653f); }
.tg-actions { display: flex; gap: 6px; flex-shrink: 0; }

.tg-modes { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; padding: 12px; }
@container (max-width: 560px) { .tg-modes { grid-template-columns: minmax(0, 1fr); } }
.tg-mode { position: relative; display: flex; flex-direction: column; gap: 6px; padding: 12px; border-radius: 9px; border: 1px solid var(--color-border); background: var(--color-panel); text-align: left; font: inherit; color: var(--color-fg-muted); transition: border-color .15s, background .15s; }
.tg-mode:hover { border-color: var(--color-border-strong); }
.tg-mode input { position: absolute; opacity: 0; pointer-events: none; }
.tg-mode-head { display: flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 600; color: var(--color-fg); }
.tg-mode-head .tg-icon { color: var(--color-fg-subtle); }
.tg-mode-desc { font-size: 11px; line-height: 1.5; }
.tg-radio { margin-left: auto; width: 14px; height: 14px; border-radius: 50%; border: 1px solid var(--color-border-strong); flex-shrink: 0; transition: border-width .12s, border-color .12s; }
.tg-mode.tg-on { border-color: var(--color-accent); background: color-mix(in srgb, var(--color-accent) 7%, var(--color-panel)); }
.tg-mode.tg-on .tg-mode-head .tg-icon { color: var(--color-accent); }
.tg-mode.tg-on .tg-radio { border: 4px solid var(--color-accent); }
.tg-mode:has(input:focus-visible) { box-shadow: 0 0 0 3px var(--color-accent-soft); }
.tg-toggle { display: flex; align-items: center; gap: 12px; padding: 12px 14px; border-top: 1px solid var(--color-border); }

.tg-phone { display: grid; grid-template-columns: minmax(0, 1.1fr) minmax(0, 1fr); }
@container (max-width: 620px) { .tg-phone { grid-template-columns: minmax(0, 1fr); } }
.tg-chat { padding: 18px; display: flex; flex-direction: column; align-items: flex-start; gap: 6px; background: radial-gradient(circle at 20% 0%, color-mix(in srgb, #2aabee 12%, transparent), transparent 60%), var(--color-panel); border-right: 1px solid var(--color-border); }
@container (max-width: 620px) { .tg-chat { border-right: 0; border-bottom: 1px solid var(--color-border); } }
.tg-bubble { max-width: 100%; width: 300px; padding: 9px 11px 7px; border-radius: 12px 12px 12px 4px; background: var(--color-panel-raised); border: 1px solid var(--color-border); font-size: 12px; line-height: 1.5; color: var(--color-fg); display: flex; flex-direction: column; gap: 6px; }
.tg-bubble pre { margin: 0; padding: 6px 8px; border-radius: 6px; background: var(--color-active); font-family: var(--font-mono); font-size: 11px; color: var(--color-fg-muted); white-space: pre-wrap; }
.tg-bubble i { color: var(--color-fg-subtle); font-size: 11px; }
.tg-bubble-time { align-self: flex-end; font-size: 10px; color: var(--color-fg-subtle); }
.tg-kb { width: 300px; max-width: 100%; display: grid; grid-template-columns: 1fr 1fr; gap: 4px; }
.tg-kb span { display: flex; align-items: center; justify-content: center; height: 28px; border-radius: 7px; font-size: 11px; font-weight: 500; color: var(--color-fg); background: color-mix(in srgb, var(--color-panel-raised) 80%, #2aabee 20%); }
.tg-kb span.tg-wide { grid-column: span 2; }
.tg-cmds { padding: 14px 16px; display: flex; flex-direction: column; gap: 2px; }
.tg-cmd { display: flex; align-items: baseline; gap: 10px; padding: 5px 0; font-size: 12px; color: var(--color-fg-muted); }
.tg-cmd .tg-code { min-width: 54px; text-align: center; }
.tg-cmds .tg-hint { margin-top: 8px; line-height: 1.5; align-items: flex-start; }
.tg-cmds .tg-hint .tg-icon { margin-top: 2px; }
`;

const ICONS = {
  key: "M15.5 7.5 19 4m2-2-2 2m0 0 3 3-3.5 3.5-3-3M11.4 11.6a5.5 5.5 0 1 1-7.8 7.8 5.5 5.5 0 0 1 7.8-7.8z",
  eye: "M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z",
  eyeOff: "M9.9 9.9a3 3 0 1 0 4.2 4.2M10.7 5.1A10 10 0 0 1 12 5c7 0 10 7 10 7a13 13 0 0 1-1.7 2.7M6.6 6.6A13.5 13.5 0 0 0 2 12s3 7 10 7a9.7 9.7 0 0 0 5.4-1.6M2 2l20 20",
  copy: "M9 9h11v11H9zM5 15H4V4h11v1",
  check: "M20 6 9 17l-5-5",
  lock: "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
  monitor: "M3 4h18v12H3zM8 20h8M12 16v4",
  away: "M22 2 11 13M22 2l-7 20-4-9-9-4 20-7z",
  pause: "M10 4H6v16h4zM18 4h-4v16h4z",
  refresh: "M21 12a9 9 0 1 1-3-6.7L21 8M21 3v5h-5",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  reply: "M9 17 4 12l5-5M20 18v-2a4 4 0 0 0-4-4H4",
};
/** Telegram's paper plane, filled. */
const PLANE = "M9.78 18.65l.28-4.23 7.68-6.92c.34-.31-.07-.46-.52-.19L7.74 13.3 3.64 12c-.88-.25-.89-.86.2-1.3l15.97-6.16c.73-.33 1.43.18 1.15 1.3l-2.72 12.81c-.19.91-.74 1.13-1.5.71L12.6 16.3l-1.99 1.93c-.23.23-.42.42-.83.42z";

function svg(d, filled = false) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.setAttribute("viewBox", "0 0 24 24");
  el.setAttribute("class", "tg-icon");
  if (filled) el.setAttribute("fill", "currentColor");
  else for (const [k, v] of Object.entries({ fill: "none", stroke: "currentColor", "stroke-width": "2", "stroke-linecap": "round", "stroke-linejoin": "round" })) el.setAttribute(k, v);
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", d);
  el.append(path);
  return el;
}
const icon = (name) => svg(ICONS[name]);
const logo = (small) => h("div", { class: `tg-logo${small ? " tg-small" : ""}` }, svg(PLANE, true));

const MODE_CARDS = {
  unfocused: { icon: "monitor", title: "In the background", desc: "Only while winfer isn't the active window." },
  always: { icon: "away", title: "I'm away", desc: "Every request and finish, until you send /back." },
  paused: { icon: "pause", title: "Paused", desc: "No messages until you send /back or /away." },
};
const COMMANDS = [
  ["/status", "List your agents and what they're doing"],
  ["/away", "Message me about everything"],
  ["/back", "Only while winfer is in the background"],
  ["/pause", "Stop messaging me"],
];

function mountPage(root, store, api) {
  const { settings, conn } = store;
  let draft = "";
  let busy = false;
  let error = "";
  let copied = null;
  const style = h("style", {}, CSS);
  const page = h("div", { class: "wf-page tg-page" });
  root.append(style, page);
  // Keeps the pairing countdown current.
  const ticker = setInterval(() => store.pairing() && render(), 15_000);

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

  async function copy(key, text) {
    try {
      await navigator.clipboard.writeText(text);
      copied = key;
      render();
      setTimeout(() => copied === key && ((copied = null), render()), 1500);
    } catch {
      api.toast("Couldn't copy to the clipboard", "error");
    }
  }

  const copyButton = (key, text, label) =>
    h("button", { class: "wf-button", onclick: () => copy(key, text) }, icon(copied === key ? "check" : "copy"), copied === key ? "Copied" : label);

  const connect = () => draft && act(() => store.setToken(draft).then(() => (draft = "")));

  function status() {
    if (!conn.token) return ["", "Not set up"];
    if (conn.error) return ["tg-bad", "Offline"];
    if (!conn.bot) return ["tg-wait", "Connecting…"];
    if (settings.chatId === null) return ["tg-wait", "Waiting to pair"];
    if (settings.mode === "paused") return ["", "Paused"];
    return ["tg-ok", settings.mode === "always" ? "Away mode" : "Active"];
  }

  function header() {
    const [tone, label] = status();
    return h(
      "div",
      { class: "tg-hero" },
      logo(),
      h(
        "div",
        { class: "tg-hero-text" },
        h("h1", { class: "wf-h1" }, "Telegram"),
        h("div", { class: "wf-muted" }, "Get a message when an agent needs you, and approve, decline or answer it from your phone."),
      ),
      h("span", { class: `tg-pill ${tone}` }, h("span", { class: "tg-dot" }), label),
    );
  }

  function step(n, state, title, done, ...body) {
    return h(
      "div",
      { class: `tg-step${state === "active" ? " tg-active" : state === "complete" ? " tg-complete" : ""}` },
      h("div", { class: "tg-num" }, state === "complete" ? icon("check") : String(n)),
      h("div", { class: "tg-step-body" }, h("div", { class: "tg-step-head" }, h("div", { class: "tg-step-title" }, title), state === "complete" && done), state === "active" && body),
    );
  }

  function tokenStep() {
    const input = h("input", {
      type: "password",
      placeholder: "123456789:AAH…",
      value: draft,
      spellcheck: "false",
      autocomplete: "off",
      oninput: (e) => (draft = e.target.value.trim()),
      onkeydown: (e) => e.key === "Enter" && connect(),
    });
    const reveal = h("button", { class: "tg-icon-btn", title: "Show token", onclick: () => {
      const show = input.type === "password";
      input.type = show ? "text" : "password";
      reveal.replaceChildren(icon(show ? "eyeOff" : "eye"));
      input.focus();
    } }, icon("eye"));
    return [
      h(
        "ol",
        { class: "tg-list" },
        h("li", {}, h("span", {}, "In Telegram, open a chat with ", h("b", {}, "@BotFather"), " and send ", h("span", { class: "tg-code" }, "/newbot"), ".")),
        h("li", {}, h("span", {}, "Pick a name and a username ending in ", h("span", { class: "tg-code" }, "bot"), ".")),
        h("li", {}, h("span", {}, "Paste the token it sends you below.")),
      ),
      h(
        "div",
        { class: "tg-field" },
        icon("key"),
        input,
        reveal,
        h("button", { class: "wf-button wf-primary", disabled: busy, onclick: connect }, busy ? "Checking…" : "Connect"),
      ),
      h("div", { class: "tg-hint" }, icon("lock"), "Saved in your keychain. Agents never see it."),
    ];
  }

  function pairStep() {
    const pairing = store.pairing();
    if (!conn.bot) {
      return [conn.error ? h("div", { class: "wf-error" }, conn.error) : h("div", { class: "wf-muted" }, "Connecting to your bot…")];
    }
    if (!pairing) {
      return [
        h("div", { class: "wf-muted" }, "Link your Telegram account to the bot. Only the paired account can approve or answer agents; everyone else is ignored."),
        h("div", {}, h("button", { class: "wf-button wf-primary", onclick: () => store.startPairing() }, "Get a pairing code")),
      ];
    }
    const left = pairing.expires - Date.now();
    const link = `https://t.me/${conn.bot.username}?start=${pairing.code}`;
    const command = `/start ${pairing.code}`;
    return [
      h(
        "div",
        { class: "tg-pair" },
        h(
          "div",
          { class: "tg-pair-top" },
          h("span", {}, "Send this to ", h("b", {}, `@${conn.bot.username}`), " in Telegram"),
          left > 0
            ? h("span", { class: "tg-expiry" }, `Expires in ${Math.max(1, Math.ceil(left / 60_000))} min`)
            : h("span", { class: "tg-expiry tg-expired" }, "Code expired"),
        ),
        h("div", { class: "tg-command" }, h("code", {}, "/start ", h("span", {}, pairing.code)), copyButton("cmd", command, "Copy")),
        h("div", { class: "tg-link" }, h("span", {}, "or open"), h("code", {}, link), h("button", { class: "tg-icon-btn", title: "Copy link", onclick: () => copy("link", link) }, icon(copied === "link" ? "check" : "copy"))),
      ),
      h("div", { class: "tg-hint" }, icon("refresh"), h("span", {}, "This page updates by itself once you're paired. ", h("button", { class: "wf-button wf-ghost", style: "height:20px;padding:0 4px;font-size:11px", onclick: () => store.startPairing() }, "New code"))),
    ];
  }

  function setup() {
    const hasBot = !!conn.token;
    return h(
      "div",
      { class: "wf-card tg-steps" },
      step(
        1,
        hasBot ? "complete" : "active",
        "Create a bot",
        h(
          "div",
          { class: "wf-inline", style: "gap:4px" },
          h("span", { class: "tg-step-done" }, conn.bot ? `@${conn.bot.username}` : "Token saved"),
          h("button", { class: "wf-button wf-ghost", disabled: busy, onclick: () => act(() => store.removeToken()) }, "Change"),
        ),
        ...tokenStep(),
      ),
      step(2, hasBot ? "active" : "pending", "Pair your account", null, ...pairStep()),
    );
  }

  function connection() {
    const initials = settings.userName.replace(/^@/, "").slice(0, 2).toUpperCase() || "?";
    return h(
      "div",
      { class: "tg-section" },
      h("div", { class: "tg-label" }, "Connection"),
      h(
        "div",
        { class: "wf-card" },
        h(
          "div",
          { class: "tg-who" },
          logo(true),
          h(
            "div",
            { class: "tg-who-text" },
            h("div", { class: "tg-who-name" }, conn.bot ? `@${conn.bot.username}` : "Your bot"),
            conn.error
              ? h("div", { class: "tg-who-sub tg-bad" }, h("span", { class: "tg-dot" }), conn.error)
              : conn.bot
                ? h("div", { class: "tg-who-sub tg-ok" }, h("span", { class: "tg-dot" }), "Connected · listening for replies")
                : h("div", { class: "tg-who-sub tg-wait" }, h("span", { class: "tg-dot" }), "Connecting…"),
          ),
          h(
            "button",
            {
              class: "wf-button wf-ghost wf-danger",
              disabled: busy,
              onclick: async () => {
                if (await api.confirm("Disconnect the bot? You'll have to set it up and pair again.", { okLabel: "Disconnect" })) act(() => store.removeToken());
              },
            },
            "Disconnect",
          ),
        ),
        h(
          "div",
          { class: "tg-who" },
          h("div", { class: "tg-avatar" }, initials),
          h("div", { class: "tg-who-text" }, h("div", { class: "tg-who-name" }, settings.userName || "Your account"), h("div", { class: "tg-who-sub" }, "Paired account · the only one the bot answers")),
          h(
            "div",
            { class: "tg-actions" },
            h("button", { class: "wf-button", disabled: busy || !conn.bot, onclick: () => act(() => store.sendTest().then(() => api.toast("Test message sent", "success"))) }, "Send test"),
            h("button", { class: "wf-button wf-ghost", disabled: busy, onclick: () => act(() => store.unpair()) }, "Unpair"),
          ),
        ),
      ),
    );
  }

  function when() {
    return h(
      "div",
      { class: "tg-section" },
      h("div", { class: "tg-label" }, "When to message you"),
      h(
        "div",
        { class: "wf-card" },
        h(
          "div",
          { class: "tg-modes", role: "radiogroup" },
          Object.entries(MODE_CARDS).map(([value, mode]) =>
            h(
              "label",
              { class: `tg-mode${settings.mode === value ? " tg-on" : ""}` },
              h("input", { type: "radio", name: "tg-mode", value, checked: settings.mode === value, onchange: () => store.set({ mode: value }) }),
              h("div", { class: "tg-mode-head" }, icon(mode.icon), mode.title, h("span", { class: "tg-radio" })),
              h("div", { class: "tg-mode-desc" }, mode.desc),
            ),
          ),
        ),
        h(
          "label",
          { class: "tg-toggle" },
          h("div", { class: "wf-grow wf-stack", style: "gap:2px;flex:1" }, h("div", { class: "wf-h2" }, "Finished and failed agents"), h("div", { class: "wf-subtle" }, "Also message when an agent finishes or stops with an error.")),
          h("input", { type: "checkbox", class: "wf-switch", checked: settings.notifyDone, onchange: (e) => store.set({ notifyDone: e.target.checked }) }),
        ),
      ),
    );
  }

  function phone() {
    return h(
      "div",
      { class: "tg-section" },
      h("div", { class: "tg-label" }, "From your phone"),
      h(
        "div",
        { class: "wf-card tg-phone" },
        h(
          "div",
          { class: "tg-chat" },
          h(
            "div",
            { class: "tg-bubble" },
            h("div", {}, "🟠 ", h("b", {}, "Claude"), " · api-server asks for permission"),
            h("pre", {}, "Bash(npm run migrate)"),
            h("i", {}, "Reply to this message to decline with instructions."),
            h("span", { class: "tg-bubble-time" }, "14:02"),
          ),
          h("div", { class: "tg-kb" }, h("span", {}, "✅ Approve"), h("span", {}, "❌ Decline"), h("span", { class: "tg-wide" }, "🖥 Show screen")),
        ),
        h(
          "div",
          { class: "tg-cmds" },
          COMMANDS.map(([cmd, desc]) => h("div", { class: "tg-cmd" }, h("span", { class: "tg-code" }, cmd), desc)),
          h("div", { class: "tg-hint" }, icon("reply"), "Reply to any message to answer that agent, or to give a finished one a new instruction. ⌘K → “Telegram: I'm away” works too."),
        ),
      ),
    );
  }

  function render() {
    const paired = conn.token && settings.chatId !== null;
    const parts = [header(), error && h("div", { class: "tg-alert" }, icon("alert"), error), ...(paired ? [connection(), when(), phone()] : [setup()])];
    page.replaceChildren(...parts.filter(Boolean));
  }

  render();
  const unsubscribe = store.subscribe(render);
  return () => {
    clearInterval(ticker);
    unsubscribe();
    page.remove();
    style.remove();
  };
}
