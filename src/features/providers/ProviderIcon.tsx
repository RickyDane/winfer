import { SquareTerminal } from "lucide-react";
import { PROVIDER_LOGOS } from "./logos";
import { PROVIDER_COLORS, type ProviderId } from "./providers";


export function ProviderIcon({ provider, size = 14 }: { provider: ProviderId; size?: number }) {
  const color = PROVIDER_COLORS[provider] ?? "var(--color-fg-muted)";
  const d = PROVIDER_LOGOS[provider];
  if (!d) return <SquareTerminal size={size} strokeWidth={2.25} style={{ color }} aria-hidden />;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      fillRule="evenodd"
      clipRule="evenodd"
      style={{ color, flex: "none" }}
      aria-hidden
    >
      <path d={d} />
    </svg>
  );
}
