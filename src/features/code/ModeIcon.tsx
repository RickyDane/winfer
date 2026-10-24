import { FilePen, Flame, Hand, Map as MapIcon, Rocket, type LucideIcon } from "lucide-react";

/** One glyph per permission mode: asking, editing, planning, planning unattended, anything goes. */
const ICONS: Record<string, LucideIcon> = {
  ask: Hand,
  "auto-edit": FilePen,
  plan: MapIcon,
  "yolo-plan": Rocket,
  yolo: Flame,
};

export function ModeIcon({ mode, size = 14 }: { mode: string; size?: number }) {
  const Icon = ICONS[mode] ?? Hand;
  return <Icon size={size} aria-hidden />;
}
