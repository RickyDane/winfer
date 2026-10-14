import { Check, CircleAlert, Loader2, Mic, Type } from "lucide-react";
import { useEffect } from "react";
import { cn } from "@/lib/cn";
import { cancelPending, confirmPending, useVoice } from "./voice";

/** Level meter: five bars driven by microphone RMS. */
function Meter({ level }: { level: number }) {
  const scaled = Math.min(1, level * 12);
  return (
    <div className="flex h-4 items-center gap-0.5">
      {[0.15, 0.35, 0.55, 0.75, 0.9].map((threshold, i) => (
        <span
          key={i}
          className={cn("w-1 rounded-full transition-all duration-75", scaled > threshold ? "bg-accent" : "bg-border-strong")}
          style={{ height: `${30 + i * 15}%` }}
        />
      ))}
    </div>
  );
}

export function VoiceHUD() {
  const { phase, mode, level, transcript, message, pending, timings } = useVoice();

  useEffect(() => {
    if (phase !== "confirm") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter") confirmPending();
      else if (e.key === "Escape") cancelPending();
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, [phase]);

  if (phase === "idle") return null;
  const dictation = mode === "dictation";

  return (
    <div className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center">
      <div className="anim-rise pointer-events-auto flex max-w-[640px] min-w-[320px] flex-col gap-2 rounded-2xl border border-border-strong bg-panel-raised/95 px-4 py-3 shadow-2xl backdrop-blur">
        <div className="flex items-center gap-3 text-[13px]">
          {phase === "listening" && (
            <>
              {dictation ? <Type size={15} className="text-accent" /> : <Mic size={15} className="text-accent" />}
              <Meter level={level} />
              <span className="text-fg">{dictation ? "Dictating…" : "Listening…"}</span>
              <span className="ml-auto text-xs text-fg-subtle">release to {dictation ? "type" : "run"}</span>
            </>
          )}
          {(phase === "transcribing" || phase === "deciding") && (
            <>
              <Loader2 size={15} className="animate-spin text-accent" />
              <span className="truncate text-fg">{transcript ? `“${transcript}”` : "Transcribing…"}</span>
              {message && <span className="ml-auto shrink-0 text-xs text-fg-subtle">{message}</span>}
            </>
          )}
          {phase === "done" && (
            <>
              <Check size={15} className="text-success" />
              <span className="truncate text-fg">{message}</span>
              {timings.sttMs !== undefined && (
                <span className="ml-auto shrink-0 font-mono text-[11px] text-fg-subtle">
                  {timings.sttMs}ms{timings.decideMs !== undefined && ` + ${timings.decideMs}ms`}
                </span>
              )}
            </>
          )}
          {phase === "error" && (
            <>
              <CircleAlert size={15} className="text-warning" />
              <span className="text-fg">{message}</span>
            </>
          )}
          {phase === "confirm" && pending && (
            <>
              <CircleAlert size={15} className="text-warning" />
              <span className="truncate text-fg-muted">“{pending.transcript}”</span>
            </>
          )}
        </div>
        {phase === "done" && transcript && <div className="truncate text-xs text-fg-subtle">“{transcript}”</div>}
        {phase === "confirm" && pending && (
          <div className="flex flex-wrap items-center gap-2">
            <button onClick={() => confirmPending()} className="rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-white hover:opacity-90">
              {message} <span className="opacity-70">↩</span>
            </button>
            {pending.decisions.length === 1 &&
              pending.decisions[0].alternatives.map((alt) => (
                <button
                  key={alt.tool.id}
                  onClick={() => confirmPending({ ...pending.decisions[0], tool: alt.tool, confidence: alt.probability, alternatives: [] })}
                  className="rounded-md border border-border-strong px-2.5 py-1 text-xs text-fg hover:bg-hover"
                >
                  {alt.tool.title}
                </button>
              ))}
            <button onClick={cancelPending} className="ml-auto rounded-md px-2 py-1 text-xs text-fg-subtle hover:bg-hover">
              Cancel <span className="opacity-70">esc</span>
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
