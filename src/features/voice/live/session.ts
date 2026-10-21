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
}

export const useLiveVoice = create<LiveVoiceState>(() => ({ state: "off", muted: false, lines: [], tool: null, error: null, auth: null }));

const MAX_LINES = 12;
const addLine = (line: LiveLine) => useLiveVoice.setState((s) => ({ lines: [...s.lines, line].slice(-MAX_LINES) }));

interface Connection {
  pc: RTCPeerConnection;
  dc: RTCDataChannel;
  mic: MediaStream;
  audio: HTMLAudioElement;
  unsubscribe: (() => void)[];
  responding: boolean;
  userSpeaking: boolean;
  /** Events waiting for the model to be free. */
  notes: string[];
  closed: boolean;
}

let conn: Connection | null = null;

function send(event: Record<string, unknown>) {
  if (conn?.dc.readyState === "open") conn.dc.send(JSON.stringify(event));
}

async function sessionConfig() {
  const { voiceConfig } = useWorkspaces.getState();
  return {
    type: "realtime",
    instructions: liveInstructions(toolContext()),
    tools: await voiceTools(),
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
    case "error":
      // Cancelling a response that already ended is harmless.
      if (e.error?.code === "response_cancel_not_active") break;
      console.warn("realtime error", e.error);
      addLine({ who: "tool", text: e.error?.message ?? "Realtime error" });
      break;
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
  useLiveVoice.setState({ state: "connecting", error: null, lines: [], tool: null, muted: false });
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
    pc = new RTCPeerConnection();
    const audio = document.createElement("audio");
    audio.autoplay = true;
    pc.ontrack = (ev) => (audio.srcObject = ev.streams[0]);
    mic.getAudioTracks().forEach((t) => pc!.addTrack(t, mic!));
    const dc = pc.createDataChannel("oai-events");
    const c: Connection = { pc, dc, mic, audio, unsubscribe: [], responding: false, userSpeaking: false, notes: [], closed: false };
    conn = c;
    dc.onmessage = (ev) => onEvent(String(ev.data));
    dc.onopen = async () => {
      send({ type: "session.update", session: await sessionConfig() });
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
