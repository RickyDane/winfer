// Run against the built executable, especially the GUI-subsystem Windows release binary.
// Unit tests don't exercise its real startup or inherited stdin/stdout handles.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { mkdtemp, mkdir, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

assert(process.argv[2], "Usage: node scripts/serve-smoke.mjs <winfer executable>");
const binary = resolve(process.argv[2]);
const sandbox = await realpath(await mkdtemp(join(tmpdir(), "winfer-serve-")));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.toUpperCase().startsWith("WINFER_") && k.toUpperCase() !== "PATH"));
const emptyPath = join(sandbox, "bin");
await mkdir(emptyPath);
Object.assign(env, { HOME: sandbox, USERPROFILE: sandbox, APPDATA: sandbox, LOCALAPPDATA: sandbox, XDG_DATA_HOME: sandbox, PATH: emptyPath });
const events = new EventEmitter();
const received = [];
const processes = [];
let child, closed, lines, session;
let stderr = "";
let fail;
const failure = new Promise((_, reject) => { fail = reject; });
function launch() {
  received.length = 0;
  child = spawn(binary, ["code", "--serve", "--model", "claude/sonnet"], {
    cwd: sandbox, env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
  });
  child.on("error", fail);
  closed = once(child, "close");
  processes.push({ child, closed });
  child.stderr.setEncoding("utf8").on("data", (text) => { stderr = (stderr + text).slice(-4000); });
  lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    try {
      received.push(JSON.parse(line));
      events.emit("event");
    } catch {
      fail(new Error(`Non-JSON protocol output: ${line.slice(0, 200)}`));
    }
  });
}
function next(type) {
  return new Promise((done) => {
    const check = () => {
      const i = received.findIndex((e) => e.type === type);
      if (i < 0) return;
      events.off("event", check);
      done(received.splice(i, 1)[0]);
    };
    events.on("event", check);
    check();
  });
}
function send(cmd) { child.stdin.write(JSON.stringify(cmd) + "\n"); }
async function finish() {
  child.stdin.end();
  assert.equal((await closed)[0], 0, stderr);
  lines.close();
}
const timer = setTimeout(() => fail(new Error(`Serve protocol timed out. stderr: ${stderr}`)), 30_000);
try {
  await Promise.race([failure, (async () => {
    launch();
    session = (await next("ready")).session;
    const transcript = "Long Unicode transcript · 模型\n".repeat(6000);
    // With an empty PATH there are no paid/logged-in agent CLIs. Even an empty model
    // response must arrive; the previous broken transport left this pending forever.
    assert(Array.isArray((await next("models")).models));
    // Let the app create its own saved session. Windows Known Folder resolution varies
    // with profile redirection, so don't guess a data path and inject a fixture there.
    send({ type: "send", text: transcript });
    send({ type: "sessions" });
    await next("sessions");
    await finish();
    launch();
    await next("ready");
    send({ type: "sessions" });
    assert((await next("sessions")).sessions.some((s) => s.id === session));
    send({ type: "resume", id: session });
    assert.equal((await next("restore")).transcript[0].text, transcript);
    assert.equal((await next("ready")).session, session);
    send({ type: "send", text: "Input pipe smoke test" });
    send({ type: "sessions" });
    await next("sessions"); // Barrier: the preceding chat command has been consumed.
    await finish();
    launch();
    await next("ready");
    send({ type: "resume", id: session });
    assert.equal((await next("restore")).transcript.at(-1).text, "Input pipe smoke test");
    await finish();
    console.log("PASS: executable startup, models, sessions, Unicode resume and chat input over pipes");
  })()]);
} finally {
  clearTimeout(timer);
  for (const process of processes) {
    if (process.child.exitCode === null) process.child.kill();
    await process.closed.catch(() => {});
  }
  lines?.close();
  // If Known Folders ignored our sandbox overrides, remove only our generated session.
  if (process.platform === "win32" && process.env.APPDATA && session) {
    await rm(join(process.env.APPDATA, "dev.winfer.app", "harness", "sessions", `${session}.json`), { force: true });
  }
  await rm(sandbox, { recursive: true, force: true });
}
