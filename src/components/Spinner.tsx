import { useSyncExternalStore } from "react";
import { cn } from "@/lib/cn";

/** The "dots" spinner from sindresorhus/cli-spinners: braille frames at 80ms. */
const FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const INTERVAL = 80;

// One shared clock so every spinner on screen turns in step and only one timer runs.
let frame = 0;
let timer: ReturnType<typeof setInterval> | undefined;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  timer ??= setInterval(() => {
    frame = (frame + 1) % FRAMES.length;
    listeners.forEach((l) => l());
  }, INTERVAL);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== undefined) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

const getFrame = () => frame;

/** Loading indicator for async and background work: spinning braille dots, sized like a lucide icon. */
export function Spinner({ size = 14, className, "aria-label": label }: { size?: number; className?: string; "aria-label"?: string }) {
  const i = useSyncExternalStore(subscribe, getFrame);
  return (
    <span
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={cn("inline-flex shrink-0 items-center justify-center font-mono leading-none select-none", className)}
      style={{ width: size, height: size, fontSize: size * 1.15 }}
    >
      {FRAMES[i]}
    </span>
  );
}
