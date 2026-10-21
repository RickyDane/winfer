/**
 * Live voice: a continuous spoken conversation with OpenAI's realtime model over WebRTC. The
 * model hears the mic, talks back, and runs winfer's tools (tools.ts) through the data channel.
 * Its client secret comes from src-tauri/src/realtime.rs.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { toolContext } from "@/features/palette/entries";
import { logActivity } from "@/stores/activity";
import { providerName, useWorkspaces, type AgentStatus } from "@/stores/workspaces";
import { useBrowser } from "@/features/browser/browser";
import { useVoice } from "../voice";
import { liveInstructions } from "./instructions";
import { END_CONVERSATION, resetVoiceTools, runVoiceFunction, voiceTools } from "./tools";

const CALLS_URL = "https://api.openai.com/v1/realtime/calls";
export const DEFAULT_LIVE_MODEL = "gpt-realtime";
export const DEFAULT_LIVE_VOICE = "marin";
export const LIVE_VOICES = ["marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"];

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
  /** Connection health, to tell where audio stops when it does. */
  diag: Diagnostics;
}

export interface Diagnostics {
  micLabel: string;
  /** Audio bytes sent to / received from OpenAI. */
  sent: number;
  received: number;
  events: number;
  lastEvent: string;
  /** Recent distinct server event types, newest last. */
  trail: string[];
  /** Why the speaker isn't playing (autoplay blocked), if so. */
  playback: string | null;
}

const NO_DIAG: Diagnostics = { micLabel: "", sent: 0, received: 0, events: 0, lastEvent: "", trail: [], playback: null };

export const useLiveVoice = create<LiveVoiceState>(() => ({ state: "off", muted: false, lines: [], tool: null, error: null, auth: null, level: 0, diag: NO_DIAG }));

const setDiag = (patch: Partial<Diagnostics>) => useLiveVoice.setState((s) => ({ diag: { ...s.diag, ...patch } }));

const MAX_LINES = 12;
const addLine = (line: LiveLine) => useLiveVoice.setState((s) => ({ lines: [...s.lines, line].slice(-MAX_LINES) }));

interface Connection {
  pc: RTCPeerConnection;
  dc: RTCDataChannel;
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
let toolsDropped = false;

function send(event: Record<string, unknown>) {
  if (conn?.dc.readyState === "open") conn.dc.send(JSON.stringify(event));
}

async function sessionConfig() {
  const { voiceConfig } = useWorkspaces.getState();
  return {
    type: "realtime",
    instructions: liveInstructions(toolContext()),
    tools: toolsDropped ? [] : await voiceTools(),
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
    if (!conn || conn.closed) return;
    send({ type: "session.update", session: await sessionConfig() });
  }, delay);
}

/** Tells the model about something that happened in winfer; it decides whether to speak. */
function note(text: string) {
  if (!conn) return;
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
  console.debug("[live voice]", e.type, e);
  const { diag } = useLiveVoice.getState();
  const type = String(e.type);
  // Audio deltas and rate limits would drown out the events that tell what happened.
  const noisy = /delta$|rate_limits/.test(type);
  setDiag({ events: diag.events + 1, lastEvent: type, trail: noisy || diag.trail.at(-1) === type ? diag.trail : [...diag.trail, type].slice(-8) });
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
        const message = e.response?.status_details?.error?.message ?? "The response failed.";
        addLine({ who: "tool", text: message });
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
      const param = String(e.error?.param ?? "");
      addLine({ who: "tool", text: `Error${param ? ` (${param})` : ""}: ${e.error?.message ?? "Realtime error"}` });
      // A tool OpenAI rejects shouldn't cost the whole session: carry on without tools.
      if (param.startsWith("session.tools") && !toolsDropped) {
        toolsDropped = true;
        void sessionConfig().then((session) => send({ type: "session.update", session: { ...session, tools: [] } }));
      }
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

/** Mic level meter and send/receive counters, so a silent mic or speaker shows in the HUD. */
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
  const stats = window.setInterval(async () => {
    if (c.closed) return;
    let sent = 0;
    let received = 0;
    (await c.pc.getStats().catch(() => new Map())).forEach((r: { type: string; kind?: string; mediaType?: string; bytesSent?: number; bytesReceived?: number }) => {
      const audio = (r.kind ?? r.mediaType) === "audio";
      if (r.type === "outbound-rtp" && audio) sent += r.bytesSent ?? 0;
      if (r.type === "inbound-rtp" && audio) received += r.bytesReceived ?? 0;
    });
    setDiag({ sent, received });
  }, 1000);
  c.unsubscribe.push(() => clearInterval(stats));
}

/** Plays the model's voice; WebKit may give the track without a stream and wants the element in the page. */
function playRemote(c: Connection, ev: RTCTrackEvent) {
  c.audio.srcObject = ev.streams[0] ?? new MediaStream([ev.track]);
  c.audio
    .play()
    .then(() => setDiag({ playback: null }))
    .catch((err) => setDiag({ playback: `Speaker blocked: ${err instanceof Error ? err.message : String(err)}` }));
}

/** Retries playback from a click (for when autoplay was blocked). */
export function resumeLiveAudio() {
  if (!conn) return;
  void conn.ctx?.resume();
  conn.audio.play().then(() => setDiag({ playback: null }), () => {});
}

function watchApp(c: Connection) {
  // Panes and workspaces change the tools (pane targets) and the state in the instructions.
  c.unsubscribe.push(
    useWorkspaces.subscribe((s, prev) => {
      if (s.workspaces !== prev.workspaces || s.activeWorkspaceId !== prev.activeWorkspaceId || s.focusedPaneId !== prev.focusedPaneId) refreshSession();
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

export async function startLive(): Promise<void> {
  if (conn) return;
  useLiveVoice.setState({ state: "connecting", error: null, lines: [], tool: null, muted: false, level: 0, diag: NO_DIAG });
  toolsDropped = false;
  // The push-to-talk recorder shouldn't hold the mic at the same time.
  if (useVoice.getState().phase === "listening") void invoke("voice_stop").catch(() => {});
  let mic: MediaStream | null = null;
  let pc: RTCPeerConnection | null = null;
  try {
    const { voiceConfig } = useWorkspaces.getState();
    const secret = await invoke<{ value: string; auth: "chatgpt" | "apikey" }>("realtime_client_secret", {
      model: voiceConfig.liveModel || DEFAULT_LIVE_MODEL,
      voice: voiceConfig.liveVoice || DEFAULT_LIVE_VOICE,
    });
    useLiveVoice.setState({ auth: secret.auth });
    mic = await navigator.mediaDevices.getUserMedia({ audio: await micConstraints() });
    // Ended while connecting.
    if (useLiveVoice.getState().state !== "connecting") {
      mic.getTracks().forEach((t) => t.stop());
      return;
    }
    const track = mic.getAudioTracks()[0];
    if (!track) throw new Error("No microphone found.");
    setDiag({ micLabel: track.label || "default microphone" });
    pc = new RTCPeerConnection();
    const audio = document.createElement("audio");
    audio.autoplay = true;
    audio.style.display = "none";
    document.body.appendChild(audio);
    const c: Connection = { pc, dc: null as unknown as RTCDataChannel, mic, audio, ctx: null, unsubscribe: [], responding: false, userSpeaking: false, notes: [], closed: false };
    pc.ontrack = (ev) => playRemote(c, ev);
    pc.addTransceiver(track, { direction: "sendrecv", streams: [mic] });
    const dc = pc.createDataChannel("oai-events");
    c.dc = dc;
    conn = c;
    monitor(c);
    dc.onmessage = (ev) => onEvent(String(ev.data));
    dc.onopen = async () => {
      send({ type: "session.update", session: await sessionConfig() });
      // A short greeting: the user knows it's live, and hears whether the speaker works.
      send({ type: "response.create", response: { instructions: "Greet the user in a few words and ask what to do. Speak in the user's language if known, else English." } });
      setState("listening");
      watchApp(c);
      logActivity(useWorkspaces.getState().activeWorkspaceId, "Live voice started");
    };
    pc.onconnectionstatechange = () => {
      if (c.closed) return;
      if (pc!.connectionState === "failed" || pc!.connectionState === "closed") fail("The live voice connection dropped.");
    };
    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    const res = await fetch(CALLS_URL, { method: "POST", body: offer.sdp, headers: { Authorization: `Bearer ${secret.value}`, "Content-Type": "application/sdp" } });
    if (!res.ok) throw new Error(`OpenAI ${res.status}: ${(await res.text()).slice(0, 200)}`);
    await pc.setRemoteDescription({ type: "answer", sdp: await res.text() });
  } catch (err) {
    if (!conn) {
      mic?.getTracks().forEach((t) => t.stop());
      pc?.close();
    }
    fail(err instanceof Error ? err.message : String(err));
  }
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
  c.dc.close();
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
