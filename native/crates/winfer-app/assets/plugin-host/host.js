// winfer native's plugin runtime: runs plugins' `main` modules in a wry webview and gives them the
// same `api` as the Tauri app (src/features/plugins/plugins.ts). Calls go to Rust through
// `window.ipc.postMessage`; Rust answers through `window.__winfer.recv`.
(() => {
  "use strict";
  const AGENT_TOOL_NAME = /^[A-Za-z0-9_-]{1,64}$/;
  const ACTION_PANELS = ["browser", "android"];
  const runtimes = new Map();
  const pending = new Map();
  const inflight = new Map();
  let serial = 0;
  let panelActionSerial = 0;
  let snapshot = { state: { workspaces: [], activeWorkspaceId: "", focusedPaneId: null }, providers: {}, positions: {}, replies: {} };
  let plugins = [];
  let panelActions = [];
  let mounted = null;

  const post = (msg) => window.ipc.postMessage(JSON.stringify(msg));
  const call = (method, args = {}) =>
    new Promise((resolve, reject) => {
      const id = ++serial;
      pending.set(id, { resolve, reject });
      post({ kind: "call", id, method, args });
    });

  const isObject = (v) => !!v && typeof v === "object" && !Array.isArray(v);
  const pluginToolId = (pluginId, toolId) => `${pluginId}__${toolId}`.replace(/[^A-Za-z0-9_-]/g, "_");

  function checkParam(name, p) {
    if (!isObject(p) || typeof p.ask !== "string") throw new Error(`param “${name}” needs a kind and an ask`);
    switch (p.kind) {
      case "enum":
        if (!isObject(p.options)) throw new Error(`enum param “${name}” needs options`);
        break;
      case "count":
        if (typeof p.max !== "number") throw new Error(`count param “${name}” needs max`);
        break;
      case "level":
        if (!Array.isArray(p.levels)) throw new Error(`level param “${name}” needs levels`);
        break;
      case "span":
        if (p.slot !== "instruction" && p.slot !== "address") throw new Error(`span param “${name}” needs slot instruction or address`);
        break;
      case "bool":
        break;
      default:
        throw new Error(`param “${name}” has unknown kind “${String(p.kind)}”`);
    }
    return p;
  }

  /** A tool as Rust keeps it: palette entries and either bus commands or a code handler. */
  function toolFromSpec(pluginId, spec) {
    if (!isObject(spec)) throw new Error("a tool must be an object");
    for (const key of ["id", "title", "description"]) {
      if (typeof spec[key] !== "string" || !spec[key]) throw new Error(`tool ${spec.id ?? "?"} needs a ${key}`);
    }
    if (!spec.run && !Array.isArray(spec.commands)) throw new Error(`tool ${spec.id} needs commands`);
    const params = Object.fromEntries(Object.entries(spec.params ?? {}).map(([name, p]) => [name, checkParam(name, p)]));
    const palette = spec.palette === true ? [{ title: spec.title, args: {} }] : Array.isArray(spec.palette) ? spec.palette.map((e) => ({ title: e.title, args: e.args ?? {} })) : [];
    return {
      id: pluginToolId(pluginId, spec.id),
      toolId: spec.id,
      plugin: pluginId,
      title: spec.title,
      description: spec.description,
      risk: spec.risk === "confirm" ? "confirm" : "safe",
      params,
      code: !!spec.run,
      commands: spec.run ? null : spec.commands,
      palette,
    };
  }

  const pageInfo = (p) => ({ key: p.key, pluginId: p.pluginId, title: p.title, icon: p.icon ?? null });
  const actionInfo = (a) => ({ key: a.key, pluginId: a.pluginId, panel: a.panel, title: a.title, icon: a.icon ?? null, label: a.label ?? null, active: !!a.active });

  function report() {
    const tools = [], pages = [], agentTools = [];
    const seen = new Set();
    for (const p of plugins) {
      const rt = runtimes.get(p.id);
      if (!rt) continue;
      tools.push(...rt.tools);
      pages.push(...rt.pages.map(pageInfo));
      for (const spec of rt.agentTools) {
        if (seen.has(spec.name)) continue;
        seen.add(spec.name);
        agentTools.push({ plugin: p.id, name: spec.name, description: spec.description, inputSchema: spec.inputSchema ?? { type: "object", properties: {} } });
      }
    }
    post({ kind: "registry", tools, pages, agentTools, panelActions: panelActions.map(actionInfo) });
  }

  function setError(id, error) {
    post({ kind: "error", plugin: id, error });
  }

  function describePane(paneId) {
    const { state, providers, positions } = snapshot;
    const ws = state.workspaces.find((w) => w.panes.some((p) => p.id === paneId));
    const pane = ws?.panes.find((p) => p.id === paneId);
    if (!ws || !pane) return null;
    const position = positions[paneId] ?? 0;
    return {
      paneId,
      workspaceId: ws.id,
      workspaceName: ws.name,
      cwd: pane.worktree?.path ?? pane.cwd,
      provider: pane.provider,
      label: `${position ? `pane ${position}` : "a pane"} (${providers[pane.provider] ?? pane.provider})`,
    };
  }

  function bytesOf(data) {
    if (typeof data === "string") return Promise.resolve(new TextEncoder().encode(data));
    if (data instanceof Blob) return data.arrayBuffer().then((b) => new Uint8Array(b));
    return Promise.resolve(new Uint8Array(data));
  }

  function base64(bytes) {
    let out = "";
    for (let i = 0; i < bytes.length; i += 0x8000) out += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(out);
  }

  const manifestName = (id) => plugins.find((p) => p.id === id)?.manifest?.name ?? id;

  function makeApi(id, dir, runtime) {
    const track = (d) => {
      runtime.disposers.push(d);
      return d;
    };
    return {
      id,
      dir,
      appExe: () => call("appExe"),
      registerTool(spec) {
        const tool = toolFromSpec(id, spec);
        runtime.tools = [...runtime.tools.filter((t) => t.id !== tool.id), tool];
        if (spec.run) runtime.commands.set(`tool:${spec.id}`, spec.run);
        report();
        return track(() => {
          runtime.tools = runtime.tools.filter((t) => t !== tool);
          if (spec.run) runtime.commands.delete(`tool:${spec.id}`);
          report();
        });
      },
      registerPage(spec) {
        if (!spec?.id || !spec.title || typeof spec.mount !== "function") throw new Error("a page needs id, title and mount");
        const page = { ...spec, key: `${id}/${spec.id}`, pluginId: id };
        runtime.pages = [...runtime.pages.filter((p) => p.key !== page.key), page];
        report();
        return track(() => {
          runtime.pages = runtime.pages.filter((p) => p !== page);
          if (mounted?.page === page) unmount();
          report();
        });
      },
      registerAgentTool(spec) {
        if (!AGENT_TOOL_NAME.test(spec?.name ?? "")) throw new Error(`agent tool name “${spec?.name}” must be letters, digits, _ or -`);
        if (!spec.description || typeof spec.run !== "function") throw new Error(`agent tool ${spec.name} needs a description and run`);
        runtime.agentTools = [...runtime.agentTools.filter((t) => t.name !== spec.name), spec];
        report();
        return track(() => {
          runtime.agentTools = runtime.agentTools.filter((t) => t !== spec);
          report();
        });
      },
      registerPanelAction(spec) {
        if (!spec?.id || !spec.title || typeof spec.run !== "function") throw new Error("a panel action needs id, title and run");
        if (!ACTION_PANELS.includes(spec.panel)) throw new Error(`panel action ${spec.id}: panel must be ${ACTION_PANELS.join(" or ")}`);
        const key = `${id}/${spec.id}`;
        const s = ++panelActionSerial;
        panelActions = [...panelActions.filter((a) => a.key !== key), { ...spec, key, pluginId: id, serial: s }];
        report();
        return track(() => {
          panelActions = panelActions.filter((a) => a.serial !== s);
          report();
        });
      },
      updatePanelAction(actionId, look) {
        const key = `${id}/${actionId}`;
        const { title, icon, label, active } = look;
        const patch = Object.fromEntries(Object.entries({ title, icon, label, active }).filter(([k]) => k in look));
        panelActions = panelActions.map((a) => (a.key === key ? { ...a, ...patch } : a));
        report();
      },
      openPage: (page) => void call("execute", { command: { type: "ui.view", view: `page:${id}/${page}` } }),
      vault: {
        list: () => call("vault.list", { plugin: id }),
        set: (key, value, meta) => call("vault.set", { plugin: id, key, value: value ?? null, meta: meta ?? {} }),
        delete: (key) => call("vault.delete", { plugin: id, key }),
        reveal: (key) => call("vault.reveal", { plugin: id, key }),
        exec: ({ command, cwd, keys, timeoutSec }) => call("vault.exec", { plugin: id, command, cwd, keys, timeoutSec: timeoutSec ?? null }),
        fill: ({ workspaceId, key, selector, tab }) => call("vault.fill", { plugin: id, workspaceId, key, selector, tab: tab ?? null }),
      },
      confirm(message, opts = {}) {
        // Without an explicit caller, an agent tool running alone for this plugin is the asker.
        const calls = inflight.get(id) ?? [];
        const caller = opts.caller ?? (calls.length === 1 ? calls[0] : undefined);
        return call("confirm", {
          plugin: id,
          paneId: caller?.paneId ?? null,
          title: opts.title ?? manifestName(id),
          message: String(message),
          okLabel: opts.okLabel ?? "OK",
          kind: opts.kind ?? "warning",
        });
      },
      registerCommand(name, handler) {
        runtime.commands.set(name, handler);
        return track(() => runtime.commands.delete(name));
      },
      execute: (command) => void call("execute", { command }),
      on(type, handler) {
        const listener = (e) => {
          if (e.type === type) handler(e);
        };
        runtime.listeners.add(listener);
        return track(() => runtime.listeners.delete(listener));
      },
      state: () => structuredClone(snapshot.state),
      describePane,
      readPane: (paneId, lines = 40) => call("readPane", { paneId, lines: Math.max(1, Math.min(lines, 2000)) }),
      paneRequest: (paneId) => call("paneRequest", { paneId }),
      readFile: (path) => call("readFile", { path }),
      writeFile: async (path, data) => call("writeFile", { path, data: base64(await bytesOf(data)) }),
      revealFile: (path) => call("revealFile", { path }),
      lastReply: (paneId) => snapshot.replies[paneId] ?? null,
      usage: (provider) => call("usage", { provider: provider ?? null }),
      toast: (text, kind = "info") => void call("toast", { text: String(text), kind }),
    };
  }

  async function activate(plugin) {
    const manifest = plugin.manifest;
    const runtime = { tools: [], pages: [], agentTools: [], commands: new Map(), listeners: new Set(), disposers: [], module: null };
    runtimes.set(plugin.id, runtime);
    const problems = [];
    for (const raw of manifest.tools ?? []) {
      try {
        runtime.tools.push(toolFromSpec(plugin.id, raw));
      } catch (err) {
        problems.push(err instanceof Error ? err.message : String(err));
      }
    }
    report();
    if (manifest.main) {
      try {
        runtime.module = await import(`${plugin.moduleUrl}?v=${Date.now()}`);
        if (runtimes.get(plugin.id) !== runtime) return;
        const dispose = await runtime.module.activate?.(makeApi(plugin.id, plugin.dir, runtime));
        if (typeof dispose === "function") runtime.disposers.push(dispose);
      } catch (err) {
        problems.push(`main: ${err instanceof Error ? err.message : String(err)}`);
      }
    }
    setError(plugin.id, problems.length ? problems.join("; ") : null);
    report();
  }

  async function deactivate(id) {
    const runtime = runtimes.get(id);
    if (!runtime) return;
    runtimes.delete(id);
    if (mounted?.page.pluginId === id) unmount();
    for (const dispose of runtime.disposers.reverse()) {
      try {
        dispose();
      } catch {
        // A plugin's cleanup failing shouldn't keep the others from running.
      }
    }
    try {
      await runtime.module?.deactivate?.();
    } catch {
      // Same as above.
    }
    panelActions = panelActions.filter((a) => a.pluginId !== id);
    setError(id, null);
  }

  function findPage(key) {
    for (const rt of runtimes.values()) {
      const page = rt.pages.find((p) => p.key === key);
      if (page) return page;
    }
    return null;
  }

  function unmount() {
    if (!mounted) return;
    try {
      mounted.cleanup?.();
    } catch {
      // The page is going away either way.
    }
    mounted = null;
    const root = document.getElementById("page");
    root.replaceChildren();
    document.getElementById("error").textContent = "";
  }

  function mount(key) {
    const page = findPage(key);
    if (mounted?.page === page) return;
    unmount();
    if (!page) return;
    const root = document.getElementById("page");
    document.getElementById("scroller").scrollTop = 0;
    mounted = { page, cleanup: null };
    try {
      mounted.cleanup = page.mount(root);
    } catch (err) {
      document.getElementById("error").textContent = `This page failed to load: ${err instanceof Error ? err.message : String(err)}`;
    }
  }

  // The browser and Android panels draw outside this webview. A plugin that records one (the screen
  // recorder) reads it from a canvas[data-winfer-capture], as in the Tauri app: these stand in for them.
  // Reading a canvas's size asks Rust for that panel's frames for a while (a lease each read renews);
  // without readers its frames stop and the canvas goes back to blank (300×150, "no picture yet").
  const CAPTURE_LEASE_MS = 2000;
  const canvasWidth = Object.getOwnPropertyDescriptor(HTMLCanvasElement.prototype, "width");
  const canvasHeight = Object.getOwnPropertyDescriptor(HTMLCanvasElement.prototype, "height");
  const captures = new Map();
  for (const source of ["browser", "android"]) {
    const canvas = document.createElement("canvas");
    canvas.dataset.winferCapture = source;
    canvas.hidden = true;
    const capture = { canvas, until: 0, on: false, drawing: false };
    const read = (desc) => ({
      get() {
        lease(source);
        return desc.get.call(this);
      },
      set(v) {
        desc.set.call(this, v);
      },
    });
    Object.defineProperty(canvas, "width", read(canvasWidth));
    Object.defineProperty(canvas, "height", read(canvasHeight));
    captures.set(source, capture);
    document.body.append(canvas);
  }

  function lease(source) {
    const capture = captures.get(source);
    capture.until = Date.now() + CAPTURE_LEASE_MS;
    if (!capture.on) {
      capture.on = true;
      post({ kind: "capture", source, on: true });
    }
  }

  setInterval(() => {
    for (const [source, capture] of captures) {
      if (!capture.on || Date.now() < capture.until) continue;
      capture.on = false;
      post({ kind: "capture", source, on: false });
      canvasWidth.set.call(capture.canvas, 300);
      canvasHeight.set.call(capture.canvas, 150);
    }
  }, 500);

  async function showFrame(msg) {
    const capture = captures.get(msg.source);
    // A frame still decoding: this one is dropped, the next is newer anyway.
    if (!capture?.on || capture.drawing) return;
    capture.drawing = true;
    try {
      const bytes = Uint8Array.from(atob(msg.data), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: msg.mime }));
      const { canvas } = capture;
      if (capture.on) {
        if (canvasWidth.get.call(canvas) !== bitmap.width) canvasWidth.set.call(canvas, bitmap.width);
        if (canvasHeight.get.call(canvas) !== bitmap.height) canvasHeight.set.call(canvas, bitmap.height);
        canvas.getContext("2d").drawImage(bitmap, 0, 0);
      }
      bitmap.close();
    } catch (err) {
      console.error("panel frame failed", err);
    } finally {
      capture.drawing = false;
    }
  }

  function emit(event) {
    for (const runtime of runtimes.values()) {
      for (const listener of runtime.listeners) {
        try {
          listener(event);
        } catch (err) {
          console.error("plugin listener failed", err);
        }
      }
    }
  }

  async function answer(reqId, run) {
    try {
      const value = await run();
      post({ kind: "result", reqId, ok: true, value: value === undefined ? null : value });
    } catch (err) {
      post({ kind: "result", reqId, ok: false, value: err instanceof Error ? err.message : String(err) });
    }
  }

  window.__winfer = {
    reply(id, ok, value) {
      const p = pending.get(id);
      if (!p) return;
      pending.delete(id);
      if (ok) p.resolve(value);
      else p.reject(new Error(String(value)));
    },
    async recv(msg) {
      switch (msg.type) {
        case "load":
          for (const id of [...runtimes.keys()]) await deactivate(id);
          plugins = msg.plugins;
          await Promise.all(plugins.map(activate));
          report();
          return;
        case "state":
          snapshot = msg;
          return;
        case "event":
          return emit(msg.event);
        case "theme":
          for (const [k, v] of Object.entries(msg.vars)) document.documentElement.style.setProperty(k, v);
          document.documentElement.style.colorScheme = msg.mode;
          document.documentElement.dataset.theme = msg.id;
          document.documentElement.dataset.mode = msg.mode;
          return;
        case "frame":
          return showFrame(msg);
        case "mount":
          return msg.key ? mount(msg.key) : unmount();
        case "command":
          return answer(msg.reqId, async () => {
            const handler = runtimes.get(msg.plugin)?.commands.get(msg.command);
            if (!handler) throw new Error(`Plugin “${msg.plugin}” has no command “${msg.command}”`);
            await handler(msg.args ?? {});
          });
        case "panelAction":
          return answer(msg.reqId, async () => {
            const action = panelActions.find((a) => a.key === msg.key);
            if (action) await action.run();
          });
        case "agentTool":
          return answer(msg.reqId, async () => {
            const owner = plugins.find((p) => runtimes.get(p.id)?.agentTools.some((t) => t.name === msg.name));
            const spec = owner && runtimes.get(owner.id).agentTools.find((t) => t.name === msg.name);
            if (!spec) throw new Error(`Unknown tool ${msg.name}`);
            inflight.set(owner.id, [...(inflight.get(owner.id) ?? []), msg.caller]);
            try {
              return String(await spec.run(msg.args ?? {}, msg.caller));
            } finally {
              const rest = (inflight.get(owner.id) ?? []).filter((c) => c !== msg.caller);
              if (rest.length) inflight.set(owner.id, rest);
              else inflight.delete(owner.id);
            }
          });
      }
    },
  };
  post({ kind: "ready" });
})();
