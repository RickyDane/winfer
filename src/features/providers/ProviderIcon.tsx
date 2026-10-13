import { Asterisk, Hexagon, MousePointer2, Orbit, SquareCode, SquareTerminal, Zap, type LucideIcon } from "lucide-react";
import { PROVIDER_COLORS, type ProviderId } from "./providers";

const ICONS: Record<string, LucideIcon> = {
  claude: Asterisk,
  codex: Hexagon,
  agy: Orbit,
  grok: Zap,
  opencode: SquareCode,
  cursor: MousePointer2,
};

export function ProviderIcon({ provider, size = 14 }: { provider: ProviderId; size?: number }) {
  const Icon = ICONS[provider] ?? SquareTerminal;
  return (
    <Icon
      size={size}
      strokeWidth={2.25}
      style={{ color: PROVIDER_COLORS[provider] ?? "var(--color-fg-muted)" }}
      aria-hidden
    />
  );
}
