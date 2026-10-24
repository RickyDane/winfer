import { describe, expect, it } from "vitest";
import { modelTier } from "./names";

describe("modelTier", () => {
  it("reads the tier from the model's name", () => {
    expect(modelTier("claude/haiku")).toBe("fast");
    expect(modelTier("claude/sonnet")).toBe("balanced");
    expect(modelTier("claude/opus")).toBe("frontier");
    expect(modelTier("claude/fable")).toBe("frontier");
    expect(modelTier("codex/gpt-6-mini")).toBe("fast");
    expect(modelTier("opencode/google/gemini-3-flash")).toBe("fast");
    expect(modelTier("codex/gpt-6.1-pro-fast")).toBe("frontier");
  });

  it("falls back on the backend's description, the name winning", () => {
    expect(modelTier("codex/gpt-6.1-sol", "Codex · Frontier agentic coding model")).toBe("frontier");
    expect(modelTier("codex/gpt-6-luna", "Codex · Fast, cheap model")).toBe("fast");
    expect(modelTier("claude/haiku", "most capable")).toBe("fast");
    expect(modelTier("chatgpt/gpt-6.1-sol", "ChatGPT account · winfer tools")).toBe("balanced");
  });
});
