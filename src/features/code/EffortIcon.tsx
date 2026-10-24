import type { ModelInfo } from "./protocol";

/** Reasoning effort as rising bars: more bars, more thinking; "max" and "ultra" light them all up. */
const LEVELS: Record<string, number> = { minimal: 1, low: 2, medium: 3, high: 4, xhigh: 5, max: 5, ultra: 5 };

/** Every effort any backend knows, highest first (the picker's order: lowest next to the status line). */
const ALL_EFFORTS = ["ultra", "max", "xhigh", "high", "medium", "low", "minimal"];
/** What a backend takes when its models don't list their efforts. */
const BACKEND_EFFORTS: Record<string, string[]> = {
  claude: ["low", "medium", "high", "xhigh", "max"],
  codex: ["low", "medium", "high", "xhigh", "max"],
  chatgpt: ["minimal", "low", "medium", "high", "xhigh"],
  pi: ["minimal", "low", "medium", "high", "xhigh"],
  agy: ["low", "medium", "high"],
};

/** The efforts to offer for model `spec`, highest first, ending with "default". */
export function effortOptions(models: ModelInfo[], spec: string): string[] {
  const listed = models.find((m) => m.spec === spec)?.efforts;
  const known = listed ?? BACKEND_EFFORTS[spec.split("/")[0]];
  if (!known) return [...ALL_EFFORTS.filter((e) => e !== "ultra"), "default"];
  // Known levels in their usual order; any new one a backend reports goes on top.
  const ordered = [...known.filter((e) => !ALL_EFFORTS.includes(e)).reverse(), ...ALL_EFFORTS.filter((e) => known.includes(e))];
  return [...ordered, "default"];
}

export const EFFORT_HINTS: Record<string, string> = {
  default: "the model's own default",
  minimal: "answers almost straight away",
  low: "light thinking, fast",
  medium: "balanced",
  high: "thinks things through",
  xhigh: "extra deliberate",
  max: "as much thinking as it takes",
  ultra: "the deepest it goes, slowest",
};

export const EFFORT_LABELS: Record<string, string> = {
  default: "Default",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
  ultra: "Ultra",
};

export function EffortIcon({ effort, size = 14 }: { effort: string; size?: number }) {
  const level = LEVELS[effort] ?? 0;
  const max = effort === "max" || effort === "ultra";
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={`cw-effort${max ? " max" : ""}`} aria-hidden>
      {[0, 1, 2, 3, 4].map((i) => {
        const h = 3 + i * 2.6;
        return (
          <rect
            key={i}
            x={1 + i * 3}
            y={15 - h}
            width={2}
            height={h}
            rx={1}
            className={i < level ? "on" : level === 0 ? "dash" : "off"}
            style={{ transitionDelay: `${i * 40}ms` }}
          />
        );
      })}
    </svg>
  );
}
