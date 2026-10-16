// Screen Recorder: records the browser panel or the Android mirror to an MP4 file.
//
// Both panels draw into a <canvas> (CDP screencast frames, decoded H.264) marked with
// data-winfer-capture. A recording copies that canvas onto a fixed-size canvas of its own 30
// times a second (so tab switches, resizes or the panel closing don't break the stream; the
// video holds the last picture meanwhile) and feeds it to a MediaRecorder. The webview records
// MP4 (H.264) itself where it can; elsewhere it records WebM and ffmpeg converts it, if present.

const STORAGE = "winfer.screen-recorder";
const FPS = 30;
const MAX_SIDE = 1920;
const BITRATE = 8_000_000;

export const SOURCES = {
  browser: { title: "Browser", panel: "browser" },
  android: { title: "Android mirror", panel: "device" },
};

const MP4_TYPES = ["video/mp4;codecs=avc1.640028", "video/mp4;codecs=avc1.42E01E", "video/mp4;codecs=avc1", "video/mp4"];
const WEBM_TYPES = ["video/webm;codecs=vp9", "video/webm;codecs=vp8", "video/webm"];

/** The best container this webview records: `{ mime, ext }`, or null without MediaRecorder. */
export function pickFormat(isSupported = (t) => globalThis.MediaRecorder?.isTypeSupported?.(t) ?? false) {
  if (typeof globalThis.MediaRecorder === "undefined") return null;
  const mp4 = MP4_TYPES.find(isSupported);
  if (mp4) return { mime: mp4, ext: "mp4" };
  const webm = WEBM_TYPES.find(isSupported);
  return webm ? { mime: webm, ext: "webm" } : null;
}

/** The output size for a source of `w`×`h`: at most MAX_SIDE on the long side, even numbers (H.264 needs them). */
export function outputSize(w, h, max = MAX_SIDE) {
  const scale = Math.min(1, max / Math.max(w, h, 1));
  const even = (n) => Math.max(2, Math.round((n * scale) / 2) * 2);
  return { width: even(w), height: even(h) };
}

/** Where to draw a `sw`×`sh` picture inside `dw`×`dh`, scaled to fit and centered. */
export function fit(sw, sh, dw, dh) {
  const scale = Math.min(dw / sw, dh / sh);
  const w = sw * scale;
  const h = sh * scale;
  return { x: (dw - w) / 2, y: (dh - h) / 2, w, h };
}

const pad = (n) => String(n).padStart(2, "0");

/** "browser-2026-10-16_14-03-22.mp4" */
export function fileName(source, ext, date = new Date()) {
  const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  const time = `${pad(date.getHours())}-${pad(date.getMinutes())}-${pad(date.getSeconds())}`;
  return `${source}-${day}_${time}.${ext}`;
}

/** "1:05" or "1:02:03" */
export function duration(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

const isWindows = () => /Windows/i.test(navigator.userAgent);
const isMac = () => /Mac/i.test(navigator.userAgent);
const defaultFolder = () => (isMac() ? "~/Movies/winfer" : "~/Videos/winfer");
const join = (dir, name) => `${dir.replace(/[\\/]+$/, "")}${isWindows() ? "\\" : "/"}${name}`;
const quote = (path) => `"${path.replace(/"/g, '\\"')}"`;

function load() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE) ?? "{}");
    return { folder: saved.folder || defaultFolder(), recent: Array.isArray(saved.recent) ? saved.recent : [] };
  } catch {
    return { folder: defaultFolder(), recent: [] };
  }
}

function save(settings) {
  try {
    localStorage.setItem(STORAGE, JSON.stringify(settings));
  } catch {
    // Private storage off: settings last until the app closes.
  }
}

const findCanvas = (source) => document.querySelector(`canvas[data-winfer-capture="${source}"]`);
/** A canvas still at its default 300×150 hasn't drawn anything yet. */
const hasPicture = (c) => c && c.width > 0 && c.height > 0 && !(c.width === 300 && c.height === 150);

export function activate(api) {
  const settings = load();
  /** source → { recorder, chunks, startedAt, timer, format, canvas, live } */
  const active = new Map();
  const listeners = new Set();
  const changed = () => listeners.forEach((fn) => fn());

  /** Opens the source's panel and waits for its canvas to show a picture. */
  async function waitForCanvas(source) {
    api.execute({ type: "ui.sidePanel", panel: SOURCES[source].panel });
    const until = Date.now() + 8000;
    while (Date.now() < until) {
      const c = findCanvas(source);
      if (hasPicture(c)) return c;
      await new Promise((r) => setTimeout(r, 150));
    }
    throw new Error(
      source === "browser" ? "The browser panel isn't showing a page. Open one first." : "The Android mirror isn't showing a device. Connect one first.",
    );
  }

  async function start(source) {
    if (!SOURCES[source]) throw new Error(`Unknown source “${source}”: use browser or android.`);
    if (active.has(source)) throw new Error(`Already recording the ${SOURCES[source].title.toLowerCase()}.`);
    const format = pickFormat();
    if (!format) throw new Error("This webview can't record video (no MediaRecorder).");
    const first = await waitForCanvas(source);
    if (active.has(source)) throw new Error(`Already recording the ${SOURCES[source].title.toLowerCase()}.`);

    const size = outputSize(first.width, first.height);
    const canvas = document.createElement("canvas");
    canvas.width = size.width;
    canvas.height = size.height;
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, size.width, size.height);

    const entry = { source, format, canvas, chunks: [], startedAt: Date.now(), live: true, recorder: null, timer: 0 };
    const copy = () => {
      // Found again each tick: the panel remounts its canvas when the tab or device changes.
      const src = findCanvas(source);
      entry.live = hasPicture(src);
      if (!entry.live) return;
      const r = fit(src.width, src.height, size.width, size.height);
      ctx.fillRect(0, 0, size.width, size.height);
      ctx.drawImage(src, r.x, r.y, r.w, r.h);
    };
    copy();
    entry.timer = window.setInterval(copy, 1000 / FPS);

    const stream = canvas.captureStream(FPS);
    entry.recorder = new MediaRecorder(stream, { mimeType: format.mime, videoBitsPerSecond: BITRATE });
    entry.recorder.ondataavailable = (e) => e.data.size && entry.chunks.push(e.data);
    entry.recorder.start(1000);
    active.set(source, entry);
    changed();
    return entry;
  }

  /** Stops a recording and saves it; resolves to the saved file's path. */
  async function stop(source) {
    const entry = active.get(source);
    if (!entry) throw new Error(`Not recording the ${SOURCES[source]?.title.toLowerCase() ?? source}.`);
    active.delete(source);
    changed();
    clearInterval(entry.timer);
    await new Promise((resolve) => {
      entry.recorder.onstop = resolve;
      if (entry.recorder.state === "inactive") resolve();
      else entry.recorder.stop();
    });
    entry.recorder.stream.getTracks().forEach((t) => t.stop());
    const blob = new Blob(entry.chunks, { type: entry.format.mime.split(";")[0] });
    if (blob.size === 0) throw new Error("The recording is empty.");

    const name = fileName(source, entry.format.ext, new Date(entry.startedAt));
    let path = await api.writeFile(join(settings.folder, name), blob);
    if (entry.format.ext === "webm") path = await toMp4(path);
    settings.recent = [{ path, source, at: entry.startedAt, ms: Date.now() - entry.startedAt }, ...settings.recent].slice(0, 30);
    save(settings);
    changed();
    return path;
  }

  /** Converts a WebM recording with ffmpeg; keeps the WebM if ffmpeg isn't there or fails. */
  async function toMp4(webm) {
    const mp4 = webm.replace(/\.webm$/, ".mp4");
    const dir = webm.replace(/[\\/][^\\/]*$/, "");
    const remove = isWindows() ? `del ${quote(webm)}` : `rm -f ${quote(webm)}`;
    const command = `ffmpeg -y -loglevel error -i ${quote(webm)} -c:v libx264 -pix_fmt yuv420p -movflags +faststart ${quote(mp4)} && ${remove}`;
    try {
      const run = await api.vault.exec({ command, cwd: dir, keys: [], timeoutSec: 600 });
      if (run.code === 0) return mp4;
      api.toast("Saved as WebM: this webview can't record MP4 and ffmpeg couldn't convert it", "error");
    } catch {
      api.toast("Saved as WebM: this webview can't record MP4; install ffmpeg to get MP4s", "error");
    }
    return webm;
  }

  async function toggle(source) {
    try {
      if (active.has(source)) {
        const path = await stop(source);
        api.toast(`Saved ${path.split(/[\\/]/).pop()}`, "success");
      } else {
        await start(source);
        api.toast(`Recording the ${SOURCES[source].title.toLowerCase()}`, "info");
      }
    } catch (err) {
      api.toast(err instanceof Error ? err.message : String(err), "error");
    }
  }

  async function stopAll() {
    if (active.size === 0) return api.toast("Nothing is recording");
    for (const source of [...active.keys()]) await toggle(source);
  }

  const dispose = [];

  // --- palette and voice --------------------------------------------------------------------

  dispose.push(
    api.registerTool({
      id: "record",
      title: "Start or stop recording",
      description: "Record the browser or Android phone screen to an MP4 video, or stop recording",
      params: {
        source: { kind: "enum", ask: "Record the browser or the Android mirror?", options: { browser: "The browser panel", android: "The Android mirror" } },
      },
      palette: [
        { title: "Record browser (start / stop)", args: { source: "browser" } },
        { title: "Record Android mirror (start / stop)", args: { source: "android" } },
      ],
      run: (args) => toggle(String(args.source ?? "browser")),
    }),
    api.registerTool({
      id: "stop",
      title: "Stop recording",
      description: "Stop every screen recording and save the videos",
      palette: true,
      run: stopAll,
    }),
  );

  // --- agents -------------------------------------------------------------------------------

  const sourceSchema = { type: "string", enum: Object.keys(SOURCES), description: "browser (the browser panel) or android (the Android mirror)" };
  dispose.push(
    api.registerAgentTool({
      name: "recording_start",
      description:
        "Start recording the browser panel or the Android mirror to an MP4 video, e.g. to demo a feature. Do the steps to show, then call recording_stop.",
      inputSchema: { type: "object", properties: { source: sourceSchema }, required: ["source"] },
      async run(args) {
        const entry = await start(String(args.source));
        return `Recording the ${SOURCES[entry.source].title.toLowerCase()} (${entry.canvas.width}×${entry.canvas.height}). Call recording_stop when done.`;
      },
    }),
    api.registerAgentTool({
      name: "recording_stop",
      description: "Stop a recording started with recording_start and save it; returns the video's path.",
      inputSchema: { type: "object", properties: { source: sourceSchema }, required: ["source"] },
      async run(args) {
        const path = await stop(String(args.source));
        api.toast(`Saved ${path.split(/[\\/]/).pop()}`, "success");
        return `Saved the recording to ${path}`;
      },
    }),
  );

  // --- recording indicator ------------------------------------------------------------------

  const badge = document.createElement("div");
  badge.style.cssText =
    "position:fixed;left:50%;bottom:14px;transform:translateX(-50%);z-index:9999;display:none;gap:6px;align-items:center;" +
    "padding:4px 6px 4px 10px;border-radius:999px;background:var(--color-panel);border:1px solid var(--color-border);" +
    "box-shadow:0 4px 16px rgba(0,0,0,.25);font:12px/1.2 system-ui,sans-serif;color:var(--color-fg)";
  document.body.appendChild(badge);
  const renderBadge = () => {
    badge.style.display = active.size ? "flex" : "none";
    badge.replaceChildren(
      ...[...active.values()].map((e) => {
        const item = document.createElement("span");
        item.style.cssText = "display:inline-flex;gap:6px;align-items:center";
        const dot = document.createElement("span");
        dot.style.cssText = `width:8px;height:8px;border-radius:50%;background:${e.live ? "#ef4444" : "#a3a3a3"}`;
        const label = document.createElement("span");
        label.textContent = `${SOURCES[e.source].title} ${duration(Date.now() - e.startedAt)}${e.live ? "" : " (hidden)"}`;
        label.style.fontVariantNumeric = "tabular-nums";
        const button = document.createElement("button");
        button.textContent = "Stop";
        button.className = "wf-button wf-ghost";
        button.style.cssText = "padding:2px 8px;font-size:11px";
        button.onclick = () => toggle(e.source);
        item.append(dot, label, button);
        return item;
      }),
    );
  };
  listeners.add(renderBadge);
  const tick = window.setInterval(() => active.size && renderBadge(), 500);
  dispose.push(() => {
    clearInterval(tick);
    badge.remove();
  });

  // --- page -----------------------------------------------------------------------------------

  dispose.push(api.registerPage({ id: "recordings", title: "Recordings", icon: "video", mount: (el) => mountPage(el) }));

  function mountPage(el) {
    const format = pickFormat();
    const render = () => {
      el.replaceChildren();
      const page = h("div", "wf-page wf-stack");
      page.append(
        h("div", "wf-header", [
          h("div", "wf-stack", [
            h("h1", "wf-h1", "Recordings"),
            h(
              "p",
              "wf-muted",
              format
                ? `Records the browser panel or the Android mirror as ${format.ext === "mp4" ? "MP4 (H.264)" : "WebM, converted to MP4 with ffmpeg"}.`
                : "This webview can't record video.",
            ),
          ]),
        ]),
      );

      const controls = h("div", "wf-card wf-stack");
      for (const source of Object.keys(SOURCES)) {
        const entry = active.get(source);
        const button = h("button", `wf-button ${entry ? "wf-danger" : "wf-primary"}`, entry ? "Stop and save" : "Start recording");
        button.disabled = !format;
        button.onclick = () => toggle(source);
        controls.append(
          h("div", "wf-row", [
            h("span", "wf-grow", [
              h("strong", "", SOURCES[source].title),
              entry ? h("span", "wf-badge wf-accent", ` ● ${duration(Date.now() - entry.startedAt)}`) : "",
            ]),
            button,
          ]),
        );
      }
      page.append(controls);

      const folder = h("input", "wf-input wf-grow");
      folder.value = settings.folder;
      folder.onchange = () => {
        settings.folder = folder.value.trim() || defaultFolder();
        save(settings);
      };
      page.append(h("label", "wf-field", [h("span", "wf-subtle", "Save to"), h("div", "wf-row", [folder])]));

      page.append(h("h2", "wf-h2", "Recent"));
      if (settings.recent.length === 0) page.append(h("p", "wf-empty", "No recordings yet. Start one above, from ⌘K or ask an agent to record a demo."));
      for (const r of settings.recent) {
        const reveal = h("button", "wf-button wf-ghost", isMac() ? "Show in Finder" : "Show in folder");
        reveal.onclick = () => api.revealFile(r.path).catch((err) => api.toast(String(err), "error"));
        page.append(
          h("div", "wf-card wf-row", [
            h("div", "wf-grow wf-stack", [
              h("span", "wf-mono", r.path.split(/[\\/]/).pop()),
              h("span", "wf-subtle", `${SOURCES[r.source]?.title ?? r.source} · ${duration(r.ms)} · ${new Date(r.at).toLocaleString()}`),
            ]),
            reveal,
          ]),
        );
      }
      el.append(page);
    };
    render();
    listeners.add(render);
    const timer = window.setInterval(() => active.size && render(), 1000);
    return () => {
      listeners.delete(render);
      clearInterval(timer);
    };
  }

  return () => {
    for (const d of dispose.reverse()) d();
    // Turned off or reloaded mid-recording: save what there is.
    for (const source of [...active.keys()]) stop(source).catch(() => {});
  };
}

function h(tag, className, children) {
  const el = document.createElement(tag);
  if (className) el.className = className;
  for (const child of [children ?? []].flat()) el.append(child);
  return el;
}
