/** Speaks agent updates aloud ("Codex needs your input") when enabled in Settings. */
import { useWorkspaces } from "@/stores/workspaces";

export function speak(text: string) {
  if (!useWorkspaces.getState().voiceConfig.readback || !("speechSynthesis" in window)) return;
  const utterance = new SpeechSynthesisUtterance(text);
  utterance.rate = 1.05;
  window.speechSynthesis.cancel(); // newest update wins
  window.speechSynthesis.speak(utterance);
}
