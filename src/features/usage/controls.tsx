import { ChartColumn, ChartLine } from "lucide-react";
import { useState } from "react";
import { cn } from "@/lib/cn";

export type ChartKind = "bars" | "line";

/** A choice kept in localStorage; falls back to `fallback` when unset, unknown or unreadable. */
export function useSaved<T extends string>(key: string, options: readonly T[], fallback: T): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const saved = localStorage.getItem(key);
      return options.includes(saved as T) ? (saved as T) : fallback;
    } catch {
      return fallback;
    }
  });
  const set = (v: T) => {
    setValue(v);
    try {
      localStorage.setItem(key, v);
    } catch {
      // Not remembered; fine.
    }
  };
  return [value, set];
}

/** Small segmented control, e.g. Cost / Tokens. */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  className,
}: {
  value: T;
  options: { id: T; label: React.ReactNode; title?: string }[];
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <div className={cn("flex rounded-md border border-border p-0.5", className)}>
      {options.map((o) => (
        <button
          key={o.id}
          onClick={() => onChange(o.id)}
          title={o.title}
          aria-pressed={value === o.id}
          className={cn("flex items-center rounded px-2 py-0.5 text-[11px]", value === o.id ? "bg-active text-fg" : "text-fg-muted hover:text-fg")}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

const KINDS: { id: ChartKind; label: React.ReactNode; title: string }[] = [
  { id: "bars", label: <ChartColumn size={12} className="my-px" />, title: "Bar chart" },
  { id: "line", label: <ChartLine size={12} className="my-px" />, title: "Line chart" },
];

/** Bars or line, as two icon buttons. */
export const ChartKindToggle = ({ value, onChange }: { value: ChartKind; onChange: (kind: ChartKind) => void }) => (
  <Segmented value={value} options={KINDS} onChange={onChange} />
);

export type UsageTab = "usage" | "limits";
export const USAGE_TAB_KEY = "winfer.usageTab";

/** Which tab the Usage page in settings opens on next. */
export function setUsageTab(tab: UsageTab) {
  try {
    localStorage.setItem(USAGE_TAB_KEY, tab);
  } catch {
    // Opens on the last tab instead; fine.
  }
}
