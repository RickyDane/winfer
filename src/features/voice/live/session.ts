/**
 * Live voice: a continuous spoken conversation with an OpenAI voice model over WebRTC, which
 * operates winfer. Two ways to connect:
 * - "api": the public realtime API on the user's OpenAI API key (src-tauri/src/realtime.rs). The
 *   model calls winfer's tools (tools.ts) itself, through the data channel.
 * - "chatgpt": the ChatGPT plan's voice model, as the Codex app runs it
 *   (src-tauri/src/chatgpt_voice.rs). It can't call tools: it hands requests over, and a
 *   tool-calling model on the plan runs them (delegate.ts) and answers for it to speak.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { toolContext } from "@/features/palette/entries";
import { logActivity } from "@/stores/activity";
import { providerName, useWorkspaces, type AgentStatus } from "@/stores/workspaces";
import { useBrowser } from "@/features/browser/browser";
import { HANDOFF_RULES, liveInstructions } from "./instructions";
import { chunks, Delegate } from "./delegate";
import { END_CONVERSATION, resetVoiceTools, runVoiceFunction, voiceTools, type ToolLevel } from "./tools";

const CALLS_URL = "https://api.openai.com/v1/realtime/calls";
export const DEFAULT_LIVE_MODEL = "gpt-realtime";
export const DEFAULT_LIVE_VOICE = "marin";
export const LIVE_VOICES = ["marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"];
/** Voices of the ChatGPT plan's voice model. */
export const CHATGPT_VOICES = ["cove", "juniper", "maple", "spruce", "ember", "vale", "breeze", "arbor", "sol"];
export const DEFAULT_CHATGPT_VOICE = "cove";
/** How long to collect ICE candidates before sending the offer (the ChatGPT call doesn't trickle). */
const ICE_GATHER_MS = 1500;

export type LiveAuth = "auto" | "apikey" | "chatgpt";

export type LiveState = "off" | "connecting" | "listening" | "thinking" | "speaking" | "error";

export interface LiveLine {
  who: "user" | "assistant" | "tool";
  text: string;
}

interface LiveVoiceState {
  state: LiveState;
  muted: boolean;
  /** Recent conversation, newest last. */
  lines: LiveLine[];
  /** The tool running right now, for the HUD. */
  tool: string | null;
  error: string | null;
  auth: "chatgpt" | "apikey" | null;
  /** Microphone level 0–1, for the meter. */
  level: number;
  /** Why the model's voice isn't playing (autoplay blocked), if so. */
  playback: string | null;
}

export const useLiveVoice = create<LiveVoiceState>(() => ({ state: "off", muted: false, lines: [], tool: null, error: null, auth: null, level: 0, playback: null }));

const MAX_LINES = 12;
const addLine = (line: LiveLine) => useLiveVoice.setState((s) => ({ lines: [...s.lines, line].slice(-MAX_LINES) }));

interface Connection {
  kind: "api" | "chatgpt";
  pc: RTCPeerConnection;
  /** The API's event channel; the ChatGPT call's events come over winfer's control socket. */
  dc: RTCDataChannel | null;
  /** Runs handed-over requests (ChatGPT call). */
  delegate: Delegate | null;
  /** What the voice and the user have said in the current turns (ChatGPT call). */
  said: { user: string; assistant: string };
  mic: MediaStream;
  audio: HTMLAudioElement;
  ctx: AudioContext | null;
  unsubscribe: (() => void)[];
  responding: boolean;
  userSpeaking: boolean;
  /** Events waiting for the model to be free. */
  notes: string[];
  closed: boolean;
}

let conn: Connection | null = null;
/**
 * What an API connection tries. OpenAI can fail a session's first response with a bare "server
 * error" and then go silent, so each retry is a fresh connection with less: no tools, then no
 * extras (transcription, VAD, noise reduction).
 */
interface Attempt {
  tools: ToolLevel;
  bare: boolean;
}
const FIRST: Attempt = { tools: "full", bare: false };
let attempt: Attempt = FIRST;
/** Whether a response of this connection has succeeded (then errors aren't the setup's fault). */
let working = false;
/** A retry is on its way (the failed response and the error event both ask for one). */
let retrying = false;

function send(event: Record<string, unknown>) {
  if (conn?.dc?.readyState === "open") conn.dc.send(JSON.stringify(event));
}

async function sessionConfig() {
  const { voiceConfig } = useWorkspaces.getState();
  if (attempt.bare) return { type: "realtime", instructions: liveInstructions(toolContext(), voiceConfig.livePrompt) };
  return {
    type: "realtime",
    instructions: liveInstructions(toolContext(), voiceConfig.livePrompt),
    tools: await voiceTools(attempt.tools),
    tool_choice: "auto",
    audio: {
      input: {
        transcription: { model: "gpt-4o-mini-transcribe" },
        noise_reduction: { type: "far_field" },
        turn_detection: { type: "semantic_vad", eagerness: "auto", create_response: true, interrupt_response: true },
      },
      output: { voice: voiceConfig.liveVoice || DEFAULT_LIVE_VOICE },
    },
  };
}

let updateTimer: number | undefined;
/** Sends the current app state and tools to the model (debounced). */
function refreshSession(delay = 600) {
  clearTimeout(updateTimer);
  updateTimer = window.setTimeout(async () => {
    // A ChatGPT call keeps the session it started with; the delegate sees fresh state per request.
    if (!conn || conn.closed || conn.kind !== "api") return;
    send({ type: "session.update", session: await sessionConfig() });
  }, delay);
}

/** OpenAI errors no retry can fix (billing, auth): the message to stop with, else null. */
export function fatalMessage(error: { type?: string; code?: string; message?: string } | undefined): string | null {
  const code = `${error?.type ?? ""} ${error?.code ?? ""}`;
  if (/insufficient_quota|credit_balance|billing/.test(code))
    return "Your OpenAI API account has no credits left. Add credits at platform.openai.com → Settings → Billing (separate from a ChatGPT plan).";
  if (/invalid_api_key|authentication/.test(code)) return "OpenAI rejected the API key. Check it in Settings → Voice → Live voice.";
  return null;
}

/** The next, smaller attempt after a failed one; null when nothing is left to try. */
function nextAttempt(a: Attempt): Attempt | null {
  if (a.tools !== "none") return { ...a, tools: "none" };
  if (!a.bare) return { ...a, bare: true };
  return null;
}

const describeAttempt = (a: Attempt) => (a.bare ? "with a bare session" : "without tools");

/** Starts over on a fresh connection with less; false when there is nothing left to try. */
function degrade(): boolean {
  if (retrying) return true;
  if (!conn || working || conn.kind !== "api") return false;
  retrying = true;
  const next = nextAttempt(attempt);
  if (!next) {
    fail(
      "OpenAI fails every live voice session, even a bare one. Most often the API account has no credits left (platform.openai.com → Settings → Billing; separate from a ChatGPT plan). Otherwise try another model in Settings → Voice.",
    );
    return true;
  }
  addLine({ who: "tool", text: `Retrying ${describeAttempt(next)}…` });
  teardown();
  void startLive(next);
  return true;
}

/** Tells the model about something that happened in winfer; it decides whether to speak. */
function note(text: string) {
  // A ChatGPT call only speaks answers to handed-over requests.
  if (!conn || conn.kind !== "api") return;
  conn.notes.push(text);
  flushNotes();
}

function flushNotes() {
  if (!conn || conn.responding || conn.userSpeaking || conn.notes.length === 0) return;
  const text = conn.notes.splice(0).map((n) => `[winfer] ${n}`).join("\n");
  send({ type: "conversation.item.create", item: { type: "message", role: "user", content: [{ type: "input_text", text }] } });
  send({ type: "response.create" });
}

function setState(state: LiveState) {
  if (useLiveVoice.getState().state !== "off") useLiveVoice.setState({ state });
}

interface FunctionCall {
  type: "function_call";
  call_id: string;
  name: string;
  arguments: string;
}

async function runCalls(calls: FunctionCall[]) {
  setState("thinking");
  if (calls.some((c) => c.name === END_CONVERSATION)) {
    // Let a goodbye finish playing first.
    window.setTimeout(stopLive, 1500);
    return;
  }
  for (const call of calls) {
    let args: Record<string, unknown> = {};
    try {
      args = call.arguments ? JSON.parse(call.arguments) : {};
    } catch {
      /* the tool reports missing arguments */
    }
    useLiveVoice.setState({ tool: call.name });
    const output = await runVoiceFunction(call.name, args, () => refreshSession(0));
    useLiveVoice.setState({ tool: null });
    addLine({ who: "tool", text: `${call.name}${output.startsWith("Error") ? ` — ${output.slice(0, 120)}` : ""}` });
    logActivity(useWorkspaces.getState().activeWorkspaceId, `Live voice: ${call.name} ${summarizeArgs(args)}`.trim());
    send({ type: "conversation.item.create", item: { type: "function_call_output", call_id: call.call_id, output } });
  }
  send({ type: "response.create" });
}

const summarizeArgs = (args: Record<string, unknown>) =>
  Object.entries(args)
    .filter(([, v]) => typeof v !== "object")
    .map(([k, v]) => `${k}=${String(v).slice(0, 60)}`)
    .join(" ");

function onEvent(raw: string) {
  let e: Record<string, any>;
  try {
    e = JSON.parse(raw);
  } catch {
    return;
  }
  switch (e.type) {
    case "input_audio_buffer.speech_started":
      if (conn) conn.userSpeaking = true;
      setState("listening");
      break;
    case "input_audio_buffer.speech_stopped":
      if (conn) conn.userSpeaking = false;
      setState("thinking");
      break;
    case "conversation.item.input_audio_transcription.completed":
      if (e.transcript?.trim()) addLine({ who: "user", text: e.transcript.trim() });
      break;
    case "response.created":
      if (conn) conn.responding = true;
      break;
    case "output_audio_buffer.started":
      setState("speaking");
      break;
    case "output_audio_buffer.stopped":
    case "output_audio_buffer.cleared":
      setState("listening");
      break;
    case "response.output_audio_transcript.done":
      if (e.transcript?.trim()) addLine({ who: "assistant", text: e.transcript.trim() });
      break;
    case "response.done": {
      if (conn) conn.responding = false;
      const calls = ((e.response?.output ?? []) as { type: string }[]).filter((o): o is FunctionCall => o.type === "function_call");
      if (e.response?.status === "failed") {
        const error = e.response?.status_details?.error;
        addLine({ who: "tool", text: error?.message ?? "The response failed." });
        const fatal = fatalMessage(error);
        if (fatal) {
          fail(fatal);
          break;
        }
        // OpenAI's own errors here mostly come from a session it can't handle (too many or too
        // large tools): retry with fewer before giving up.
        if (error?.type === "server_error" || /server had an error/i.test(error?.message ?? "")) {
          if (degrade()) break;
        }
      } else if (e.response?.status === "completed") {
        working = true;
      }
      if (calls.length) void runCalls(calls);
      else {
        if (useLiveVoice.getState().state === "thinking") setState("listening");
        flushNotes();
      }
      break;
    }
    case "error": {
      // Cancelling a response that already ended is harmless.
      if (e.error?.code === "response_cancel_not_active") break;
      console.warn("realtime error", e.error);
      const fatal = fatalMessage(e.error);
      if (fatal) {
        fail(fatal);
        break;
      }
      const param = String(e.error?.param ?? "");
      addLine({ who: "tool", text: `Error${param ? ` (${param})` : ""}: ${e.error?.message ?? "Realtime error"}` });
      // A tool OpenAI rejects shouldn't cost the whole session: carry on without tools.
      if (param.startsWith("session.tools") || e.error?.type === "server_error") degrade();
      break;
    }
  }
}

/** Matches winfer's chosen microphone (a system device name) to a browser device id. */
async function micConstraints(): Promise<MediaTrackConstraints> {
  const base: MediaTrackConstraints = { echoCancellation: true, noiseSuppression: true, autoGainControl: true };
  const wanted = useWorkspaces.getState().voiceConfig.inputDevice;
  if (!wanted || !navigator.mediaDevices.enumerateDevices) return base;
  const devices = await navigator.mediaDevices.enumerateDevices().catch(() => []);
  const match = devices.find((d) => d.kind === "audioinput" && d.label && (d.label === wanted || d.label.includes(wanted)));
  return match ? { ...base, deviceId: { exact: match.deviceId } } : base;
}

/** Mic level for the HUD's meter. */
function monitor(c: Connection) {
  try {
    c.ctx = new AudioContext();
    void c.ctx.resume();
    const analyser = c.ctx.createAnalyser();
    analyser.fftSize = 512;
    c.ctx.createMediaStreamSource(c.mic).connect(analyser);
    const buf = new Float32Array(analyser.fftSize);
    const meter = window.setInterval(() => {
      analyser.getFloatTimeDomainData(buf);
      let sum = 0;
      for (const v of buf) sum += v * v;
      useLiveVoice.setState({ level: Math.sqrt(sum / buf.length) });
    }, 100);
    c.unsubscribe.push(() => clearInterval(meter));
  } catch (err) {
    console.warn("live voice meter failed", err);
  }
}

/** Plays the model's voice; WebKit may give the track without a stream and wants the element in the page. */
function playRemote(c: Connection, ev: RTCTrackEvent) {
  c.audio.srcObject = ev.streams[0] ?? new MediaStream([ev.track]);
  c.audio
    .play()
    .then(() => useLiveVoice.setState({ playback: null }))
    .catch((err) => useLiveVoice.setState({ playback: `Speaker blocked (${err instanceof Error ? err.message : String(err)}). Click to play.` }));
}

/** Retries playback from a click (for when autoplay was blocked). */
export function resumeLiveAudio() {
  if (!conn) return;
  void conn.ctx?.resume();
  conn.audio.play().then(() => useLiveVoice.setState({ playback: null }), () => {});
}

function watchApp(c: Connection) {
  // Panes and workspaces change the tools (pane targets) and the state in the instructions.
  c.unsubscribe.push(
    useWorkspaces.subscribe((s, prev) => {
      if (
        s.workspaces !== prev.workspaces ||
        s.activeWorkspaceId !== prev.activeWorkspaceId ||
        s.focusedPaneId !== prev.focusedPaneId ||
        s.voiceConfig.livePrompt !== prev.voiceConfig.livePrompt
      )
        refreshSession();
      const ws = s.workspaces.find((w) => w.id === s.activeWorkspaceId);
      const before = new Map(prev.workspaces.flatMap((w) => w.panes.map((p) => [p.id, p.status] as const)));
      ws?.panes.forEach((p, i) => {
        const was = before.get(p.id) as AgentStatus | undefined;
        if (!was || was === p.status || p.provider === "shell") return;
        const name = `${providerName(s.providers, p.provider)} (id ${p.id}, pane ${i + 1})`;
        if (p.status === "needs-input" && was !== "needs-input") note(`${name} needs input.`);
        else if (p.status === "done" && was === "working") note(`${name} finished its task.`);
        else if (p.status === "error") note(`${name} hit an error.`);
      });
    }),
  );
  c.unsubscribe.push(useBrowser.subscribe((s, prev) => s.tabs !== prev.tabs && refreshSession()));
  void listen<{ paneId: string; text: string }>("bg://note", ({ payload }) => {
    if (payload.paneId === "voice") note(payload.text.replace(/^\[winfer\]\s*/, ""));
  }).then((un) => (c.closed ? un() : c.unsubscribe.push(un)));
}

/** Which connection to use: the API key when there is one (or chosen), else the ChatGPT plan. */
async function pickKind(): Promise<"api" | "chatgpt"> {
  const choice = useWorkspaces.getState().voiceConfig.liveAuth ?? "auto";
  if (choice === "apikey") return "api";
  if (choice === "chatgpt") return "chatgpt";
  if (await invoke<boolean>("realtime_has_api_key").catch(() => false)) return "api";
  if (await invoke<boolean>("chatgpt_voice_ready").catch(() => false)) return "chatgpt";
  throw new Error("Live voice needs an OpenAI API key (Settings → Voice → Live voice) or the Codex CLI's ChatGPT login (`codex login`).");
}

export async function startLive(retry?: Attempt): Promise<void> {
  if (conn) return;
  attempt = retry ?? FIRST;
  working = false;
  retrying = false;
  // A retry keeps the lines, so the user sees what was tried.
  useLiveVoice.setState((s) => ({ state: "connecting", error: null, lines: retry ? s.lines : [], tool: null, muted: false, level: 0, playback: null }));
  let mic: MediaStream | null = null;
  let pc: RTCPeerConnection | null = null;
  try {
    const kind = retry ? "api" : await pickKind();
    useLiveVoice.setState({ auth: kind === "api" ? "apikey" : "chatgpt" });
    mic = await navigator.mediaDevices.getUserMedia({ audio: await micConstraints() });
    // Ended while connecting.
    if (useLiveVoice.getState().state !== "connecting") {
      mic.getTracks().forEach((t) => t.stop());
      return;
    }
    const track = mic.getAudioTracks()[0];
    if (!track) throw new Error("No microphone found.");
    pc = new RTCPeerConnection();
    const audio = document.createElement("audio");
    audio.autoplay = true;
    audio.style.display = "none";
    document.body.appendChild(audio);
    const c: Connection = {
      kind,
      pc,
      dc: null,
      delegate: null,
      said: { user: "", assistant: "" },
      mic,
      audio,
      ctx: null,
      unsubscribe: [],
      responding: false,
      userSpeaking: false,
      notes: [],
      closed: false,
    };
    pc.ontrack = (ev) => playRemote(c, ev);
    pc.addTransceiver(track, { direction: "sendrecv", streams: [mic] });
    pc.onconnectionstatechange = () => {
      if (c.closed) return;
      if (pc!.connectionState === "failed" || pc!.connectionState === "closed") fail("The live voice connection dropped.");
    };
    conn = c;
    monitor(c);
    if (kind === "api") await connectApi(c);
    else await connectChatgpt(c);
  } catch (err) {
    if (!conn) {
      mic?.getTracks().forEach((t) => t.stop());
      pc?.close();
    }
    fail(err instanceof Error ? err.message : String(err));
  }
}

/** The public realtime API: a client secret on the API key, events over the data channel. */
async function connectApi(c: Connection) {
  const { voiceConfig } = useWorkspaces.getState();
  const secret = await invoke<{ value: string }>("realtime_client_secret", {
    model: voiceConfig.liveModel || DEFAULT_LIVE_MODEL,
    voice: voiceConfig.liveVoice || DEFAULT_LIVE_VOICE,
  });
  const dc = c.pc.createDataChannel("oai-events");
  c.dc = dc;
  dc.onmessage = (ev) => onEvent(String(ev.data));
  dc.onopen = async () => {
    send({ type: "session.update", session: await sessionConfig() });
    // A short greeting: the user knows it's live, and hears whether the speaker works.
    send({ type: "response.create", response: { instructions: "Greet the user in a few words and ask what to do. Speak in the user's language if known, else English." } });
    setState("listening");
    watchApp(c);
    logActivity(useWorkspaces.getState().activeWorkspaceId, "Live voice started");
  };
  const offer = await c.pc.createOffer();
  await c.pc.setLocalDescription(offer);
  const res = await fetch(CALLS_URL, { method: "POST", body: offer.sdp, headers: { Authorization: `Bearer ${secret.value}`, "Content-Type": "application/sdp" } });
  if (!res.ok) throw new Error(`OpenAI ${res.status}: ${(await res.text()).slice(0, 200)}`);
  await c.pc.setRemoteDescription({ type: "answer", sdp: await res.text() });
}

/** Resolves once ICE gathering finishes, or after `ms`. */
function iceGathered(pc: RTCPeerConnection, ms = ICE_GATHER_MS): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", check);
      resolve();
    };
    const check = () => pc.iceGatheringState === "complete" && done();
    const timer = window.setTimeout(done, ms);
    pc.addEventListener("icegatheringstatechange", check);
  });
}

/** The ChatGPT plan: winfer creates the call and relays its control socket. */
async function connectChatgpt(c: Connection) {
  const { voiceConfig } = useWorkspaces.getState();
  c.delegate = new Delegate({
    onTool: (tool) => useLiveVoice.setState({ tool }),
    onResult: (name, args, output) => {
      addLine({ who: "tool", text: `${name}${output.startsWith("Error") ? ` — ${output.slice(0, 120)}` : ""}` });
      logActivity(useWorkspaces.getState().activeWorkspaceId, `Live voice: ${name} ${summarizeArgs(args)}`.trim());
    },
  });
  // Listening first, so the session's first events aren't missed.
  const unlistenEvent = await listen<string>("chatgpt-voice://event", ({ payload }) => onChatgptEvent(c, payload));
  const unlistenClosed = await listen<string>("chatgpt-voice://closed", ({ payload }) => {
    if (!c.closed) fail(`The live voice call ended${payload && payload !== "closed" ? ` (${payload})` : ""}.`);
  });
  c.unsubscribe.push(unlistenEvent, unlistenClosed, () => void invoke("chatgpt_voice_close").catch(() => {}));
  const offer = await c.pc.createOffer();
  await c.pc.setLocalDescription(offer);
  await iceGathered(c.pc);
  const instructions = `${liveInstructions(toolContext(), voiceConfig.livePrompt)}\n\n${HANDOFF_RULES}`;
  const call = await invoke<{ answerSdp: string }>("chatgpt_voice_call", {
    sdp: c.pc.localDescription?.sdp ?? offer.sdp ?? "",
    instructions,
    voice: voiceConfig.liveChatgptVoice || DEFAULT_CHATGPT_VOICE,
  });
  if (c.closed) return;
  await c.pc.setRemoteDescription({ type: "answer", sdp: call.answerSdp });
  await invoke("chatgpt_voice_join");
  setState("listening");
  watchApp(c);
  logActivity(useWorkspaces.getState().activeWorkspaceId, "Live voice started (ChatGPT plan)");
}

/** A control-socket event of the ChatGPT call. */
function onChatgptEvent(c: Connection, raw: string) {
  let e: Record<string, any>;
  try {
    e = JSON.parse(raw);
  } catch {
    return;
  }
  if (c.closed) return;
  switch (e.type) {
    case "input_transcript.added":
      c.said.user += e.item?.text ?? "";
      setState("listening");
      break;
    case "output_transcript.added":
      c.said.assistant += e.item?.text ?? "";
      setState("speaking");
      break;
    case "turn.done": {
      const role = e.turn?.role;
      if (role === "user") {
        const text = String(e.turn?.transcript || c.said.user).trim();
        if (text) addLine({ who: "user", text });
        c.said.user = "";
        setState("thinking");
      } else if (role === "assistant") {
        const text = c.said.assistant.trim();
        if (text) addLine({ who: "assistant", text });
        c.said.assistant = "";
        setState("listening");
      }
      break;
    }
    case "delegation.created": {
      const item = e.item ?? {};
      if (item.type !== "delegation" || item.target !== "client" || !item.id) break;
      const request = ((item.content ?? []) as { type: string; text?: string }[])
        .filter((p) => p.type === "input_text")
        .map((p) => p.text ?? "")
        .join("")
        .trim();
      void handOff(c, String(item.id), request || useLiveVoice.getState().lines.filter((l) => l.who === "user").at(-1)?.text || "");
      break;
    }
    case "error":
      addLine({ who: "tool", text: `Error: ${e.message ?? e.error?.message ?? "live voice error"}` });
      break;
  }
}

/** Runs a handed-over request and gives the voice the answer to speak. */
async function handOff(c: Connection, id: string, request: string) {
  setState("thinking");
  let answer: { text: string; end: boolean };
  try {
    answer = await c.delegate!.run(request);
  } catch (err) {
    answer = { text: `That didn't work: ${err instanceof Error ? err.message : String(err)}`, end: false };
  }
  if (c.closed) return;
  for (const text of chunks(answer.text)) {
    await invoke("chatgpt_voice_send", { message: { type: "delegation.context.append", delegation_item_id: id, content: [{ type: "input_text", text }] } }).catch(() => {});
  }
  // Let the goodbye play first.
  if (answer.end) window.setTimeout(stopLive, 4000);
}

function teardown() {
  const c = conn;
  conn = null;
  clearTimeout(updateTimer);
  resetVoiceTools();
  if (!c) return;
  c.closed = true;
  c.unsubscribe.forEach((u) => u());
  c.mic.getTracks().forEach((t) => t.stop());
  c.dc?.close();
  c.pc.close();
  c.audio.srcObject = null;
  c.audio.remove();
  void c.ctx?.close();
  useLiveVoice.setState({ level: 0 });
}

function fail(message: string) {
  teardown();
  useLiveVoice.setState({ state: "error", error: message, tool: null });
}

export function stopLive() {
  const wasOn = conn !== null;
  teardown();
  useLiveVoice.setState({ state: "off", tool: null, error: null });
  if (wasOn) logActivity(useWorkspaces.getState().activeWorkspaceId, "Live voice ended");
}

export const toggleLive = () => (conn || useLiveVoice.getState().state === "connecting" ? stopLive() : void startLive());


export function setLiveMuted(muted: boolean) {
  conn?.mic.getAudioTracks().forEach((t) => (t.enabled = !muted));
  useLiveVoice.setState({ muted });
}

/** Stops the model mid-sentence. */
export function interruptLive() {
  send({ type: "response.cancel" });
  send({ type: "output_audio_buffer.clear" });
}

export const isLive = () => conn !== null;
