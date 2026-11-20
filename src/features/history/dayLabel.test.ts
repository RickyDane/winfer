import { describe, expect, it } from "vitest";
import { dayLabel } from "./HistoryView";

describe("dayLabel", () => {
  it("groups merges by calendar day", () => {
    const now = new Date(2026, 10, 20, 11, 0).getTime();
    expect(dayLabel(new Date(2026, 10, 20, 0, 5).getTime(), now)).toBe("Today");
    expect(dayLabel(new Date(2026, 10, 19, 23, 59).getTime(), now)).toBe("Yesterday");
    expect(dayLabel(new Date(2026, 10, 17, 9, 0).getTime(), now)).toBe("3 days ago");
    expect(dayLabel(new Date(2026, 10, 10, 9, 0).getTime(), now)).toBe("Last week");
  });
});
