/** Reasoning effort as rising bars: more bars, more thinking; "max" lights them all up. */
const LEVELS: Record<string, number> = { minimal: 1, low: 2, medium: 3, high: 4, xhigh: 5, max: 5 };

export const EFFORT_HINTS: Record<string, string> = {
  default: "the model's own default",
  minimal: "answers almost straight away",
  low: "light thinking, fast",
  medium: "balanced",
  high: "thinks things through",
  xhigh: "extra deliberate",
  max: "as much thinking as it takes",
};

export const EFFORT_LABELS: Record<string, string> = {
  default: "Default",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
};

export function EffortIcon({ effort, size = 14 }: { effort: string; size?: number }) {
  const level = LEVELS[effort] ?? 0;
  const max = effort === "max";
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
