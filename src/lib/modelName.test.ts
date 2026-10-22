import { describe, expect, it } from "vitest";
import { displayModelName } from "./modelName";

describe("displayModelName", () => {
  it.each([
    ["codex/gpt-6.1-sol-fast", "GPT 6.1 Sol"],
    ["openai/gpt-6.1-sol", "GPT 6.1 Sol"],
    ["claude-opus-4-7", "Opus 4.7"],
    ["claude-sonnet-4-5-20250929", "Sonnet 4.5"],
    ["opus", "Opus"],
    ["opus[1m]", "Opus"],
    ["claude-opus-4-6-thinking", "Opus 4.6"],
    ["claude-3-5-haiku", "Haiku 3.5"],
    ["gemini-3.8-flash-high", "Gemini 3.8 Flash"],
    ["chatgpt/gpt-5.5-mini", "GPT 5.5 Mini"],
    ["anthropic/claude-sonnet-4-5-20250929", "Sonnet 4.5"],
    ["xai/grok-4", "Grok 4"],
    ["o3", "o3"],
    ["openrouter/moonshotai/kimi-k2", "Kimi K2"],
    ["claude-10", "Claude 10"],
    ["fast", "Fast"],
  ])("%s → %s", (input, expected) => expect(displayModelName(input)).toBe(expected));
});
