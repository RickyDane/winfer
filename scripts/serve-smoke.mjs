// Run against the built executable, especially the GUI-subsystem Windows release binary.
// Unit tests don't exercise its real startup or inherited stdin/stdout handles.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { EventEmitter, once } from "node:events";
import { mkdtemp, mkdir, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";

assert(process.argv[2], "Usage: node scripts/serve-smoke.mjs <winfer executable>");
const binary = resolve(process.argv[2]);
const sandbox = await realpath(await mkdtemp(join(tmpdir(), "winfer-serve-")));
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.toUpperCase().startsWith("WINFER_") && k.toUpperCase() !== "PATH"));
const emptyPath = join(sandbox, "bin");
await mkdir(emptyPath);
Object.assign(env, { HOME: sandbox, USERPROFILE: sandbox, APPDATA: sandbox, LOCALAPPDATA: sandbox, XDG_DATA_HOME: sandbox, PATH: emptyPath });
// Windows Known Folders ignore APPDATA overrides. Use a uniquely named fixture there
// and remove only that file; never overwrite or clear the user's existing sessions.
const data = process.platform === "win32" ? process.env.APPDATA
  : process.platform === "darwin" ? join(sandbox, "Library", "Application Support") : sandbox;
assert(data, "Windows APPDATA must be set");
const session = `resume-smoke-${randomUUID()}`;
const file = join(data, "dev.winfer.app", "harness", "sessions", `${session}.json`);
const child = spawn(binary, ["code", "--serve", "--model", "claude/sonnet"], {
  cwd: sandbox, env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
});
const closed = once(child, "close");
const events = new EventEmitter();
const received = [];
let stderr = "";
child.stderr.setEncoding("utf8").on("data", (text) => { stderr = (stderr + text).slice(-4000); });
let fail;
const failure = new Promise((_, reject) => { fail = reject; });
child.on("error", fail);
const lines = createInterface({ input: child.stdout });
lines.on("line", (line) => {
  try {
    received.push(JSON.parse(line));
    events.emit("event");
  } catch {
    fail(new Error(`Non-JSON protocol output: ${line.slice(0, 200)}`));
  }
});
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
const timer = setTimeout(() => fail(new Error(`Serve protocol timed out. stderr: ${stderr}`)), 30_000);
try {
  await Promise.race([failure, (async () => {
    const ready = await next("ready");
    const transcript = "Long Unicode transcript · 模型\n".repeat(6000);
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, JSON.stringify({
      id: session, model: "claude/sonnet", cwd: ready.cwd,
      title: "Resume smoke test", updated: 1, transcript: [{ role: "user", text: transcript }],
    }));
    // With an empty PATH there are no paid/logged-in agent CLIs. Even an empty model
    // response must arrive; the previous broken transport left this pending forever.
    assert(Array.isArray((await next("models")).models));
    send({ type: "sessions" });
    assert((await next("sessions")).sessions.some((s) => s.id === session));
    send({ type: "resume", id: session });
    assert.equal((await next("restore")).transcript[0].text, transcript);
    assert.equal((await next("ready")).session, session);
    send({ type: "send", text: "Input pipe smoke test" });
    send({ type: "sessions" });
    await next("sessions"); // Barrier: the preceding chat command has been consumed.
    child.stdin.end();
    assert.equal((await closed)[0], 0, stderr);
    const saved = JSON.parse(await readFile(file, "utf8"));
    assert.equal(saved.transcript.at(-1).text, "Input pipe smoke test");
    console.log("PASS: executable startup, models, sessions, Unicode resume and chat input over pipes");
  })()]);
} finally {
  clearTimeout(timer);
  if (child.exitCode === null) child.kill();
  await closed.catch(() => {});
  lines.close();
  await rm(file, { force: true });
  await rm(sandbox, { recursive: true, force: true });
}
