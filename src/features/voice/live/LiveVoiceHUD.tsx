import { AudioLines, CircleAlert, Mic, MicOff, PhoneOff, Wrench, X } from "lucide-react";
import { Spinner } from "@/components/Spinner";
import { cn } from "@/lib/cn";
import { resumeLiveAudio, setLiveMuted, startLive, stopLive, useLiveVoice, type LiveState } from "./session";

const LABEL: Record<LiveState, string> = {
  off: "",
  connecting: "Connecting…",
  listening: "Listening",
  thinking: "Thinking…",
  speaking: "Speaking",
  error: "Live voice stopped",
};

/** Mic level bars. */
function Meter({ level }: { level: number }) {
  const scaled = Math.min(1, level * 12);
  return (
    <div className="flex h-3.5 items-center gap-0.5" title="Microphone level">
      {[0.1, 0.3, 0.5, 0.7, 0.9].map((t, i) => (
        <span key={i} className={cn("w-0.5 rounded-full", scaled > t ? "bg-accent" : "bg-border-strong")} style={{ height: `${30 + i * 15}%` }} />
      ))}
    </div>
  );
}

/** The live conversation: state, recent lines, the running tool, mute and hang-up. */
export function LiveVoiceHUD() {
  const { state, muted, lines, tool, error, auth, level, playback } = useLiveVoice();
  if (state === "off") return null;
  const recent = lines.slice(-4);

  return (
    <div className="pointer-events-none fixed right-4 bottom-6 z-50 flex justify-end">
      <div className="anim-rise pointer-events-auto flex w-[360px] flex-col gap-2 rounded-2xl border border-border-strong bg-panel-raised/95 px-4 py-3 shadow-2xl backdrop-blur">
        <div className="flex items-center gap-2 text-[13px]">
          {state === "connecting" || state === "thinking" ? (
            <Spinner size={15} className="text-accent" />
          ) : state === "error" ? (
            <CircleAlert size={15} className="text-warning" />
          ) : (
            <AudioLines size={15} className={cn("text-accent", state === "speaking" && "animate-pulse")} />
          )}
          <span className="text-fg">{muted && state !== "error" ? "Muted" : LABEL[state]}</span>
          {state !== "error" && state !== "connecting" && !muted && <Meter level={level} />}
          {auth && state !== "error" && <span className="text-[11px] text-fg-subtle">{auth === "chatgpt" ? "ChatGPT" : "API key"}</span>}
          <div className="ml-auto flex items-center gap-1">
            {state === "error" ? (
              <>
                <button onClick={() => void startLive()} className="rounded-md px-2 py-1 text-xs text-fg-muted hover:bg-hover hover:text-fg">
                  Retry
                </button>
                <button onClick={stopLive} title="Close" className="rounded-md p-1 text-fg-subtle hover:bg-hover hover:text-fg">
                  <X size={14} />
                </button>
              </>
            ) : (
              <>
                <button
                  onClick={() => setLiveMuted(!muted)}
                  title={muted ? "Unmute" : "Mute"}
                  className={cn("rounded-md p-1 hover:bg-hover", muted ? "text-warning" : "text-fg-subtle hover:text-fg")}
                >
                  {muted ? <MicOff size={14} /> : <Mic size={14} />}
                </button>
                <button onClick={stopLive} title="End conversation" className="rounded-md p-1 text-fg-subtle hover:bg-hover hover:text-danger">
                  <PhoneOff size={14} />
                </button>
              </>
            )}
          </div>
        </div>
        {error && <div className="text-xs text-fg-muted">{error}</div>}
        {playback && state !== "error" && (
          <button onClick={resumeLiveAudio} className="text-left text-xs text-warning hover:underline">
            {playback}
          </button>
        )}
        {tool && (
          <div className="flex items-center gap-1.5 text-xs text-fg-muted">
            <Wrench size={12} /> <span className="font-mono">{tool}</span>
          </div>
        )}
        {recent.length > 0 && (
          <div className="flex flex-col gap-1 text-xs">
            {recent.map((l, i) => (
              <div key={i} className={cn("line-clamp-2", l.who === "user" ? "text-fg" : l.who === "assistant" ? "text-fg-muted" : "font-mono text-[11px] text-fg-subtle")}>
                {l.who === "user" ? "You: " : l.who === "assistant" ? "" : "› "}
                {l.text}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
