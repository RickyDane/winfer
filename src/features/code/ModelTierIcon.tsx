import { Brain, Feather, Scale } from "lucide-react";
import { modelTier, type ModelTier } from "./names";

const TIERS: Record<ModelTier, { icon: typeof Brain; hint: string }> = {
  fast: { icon: Feather, hint: "Fast and light: exploring, searching, small fixes" },
  balanced: { icon: Scale, hint: "Balanced: everyday coding" },
  frontier: { icon: Brain, hint: "Most capable: hard problems and orchestration" },
};

/** What a model is good for, as an icon: a feather (fast), scales (balanced) or a brain (most capable). */
export function ModelTierIcon({ spec, detail, size = 14 }: { spec: string; detail?: string; size?: number }) {
  const tier = modelTier(spec, detail);
  const { icon: Icon, hint } = TIERS[tier];
  return (
    <span className={`cw-tier cw-tier-${tier}`} title={hint} aria-label={hint} role="img">
      <Icon size={size} />
    </span>
  );
}
