import { describe, expect, it } from "vitest";
import { answerQuestion, answersText, askQuestions, dismissQuestions, parseQuestions, questionRequest, questionsFor } from "./questions";

const qs = parseQuestions([
  { header: "DB", question: "Which database?", options: [{ label: "Postgres", description: "server" }, { label: "SQLite" }] },
  { question: "Tests?", options: ["Yes", "No"], multiSelect: true },
  { question: "  ", options: [] },
]);

describe("ask_user questions", () => {
  it("parses questions, dropping empty ones", () => {
    expect(qs).toHaveLength(2);
    expect(qs[1]).toEqual({ header: undefined, question: "Tests?", options: [{ label: "Yes" }, { label: "No" }], multiSelect: true });
  });

  it("answers one question at a time and resolves with all answers", async () => {
    const done = askQuestions("p1", qs);
    const pending = questionsFor("p1")!;
    const req = questionRequest(pending);
    expect(req.text).toBe("DB (1/2)\nWhich database?");
    expect(req.options?.map((o) => o.label)).toEqual(["Postgres", "SQLite", "Type something"]);
    expect(answerQuestion(pending.id, "SQLite")).toBe(true);
    expect(questionRequest(questionsFor("p1")!).text).toContain("Tests?");
    answerQuestion(pending.id, "Yes, No");
    const answers = await done;
    expect(answers).toEqual(["SQLite", "Yes, No"]);
    expect(answersText(qs, answers!)).toContain('"Which database?" → "SQLite"');
    expect(questionsFor("p1")).toBeUndefined();
  });

  it("resolves null when dismissed", async () => {
    const done = askQuestions("p2", qs);
    dismissQuestions(questionsFor("p2")!.id);
    expect(await done).toBeNull();
  });
});
