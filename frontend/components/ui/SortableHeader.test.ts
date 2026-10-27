import { describe, expect, it } from "vitest";
import { compareSortValues } from "./SortableHeader";

const sorted = (values: (string | null)[], direction: "asc" | "desc") =>
  [...values].sort((a, b) => compareSortValues(a, b, direction));

describe("compareSortValues", () => {
  it("orders digit runs as numbers, so #10 follows #5", () => {
    expect(sorted(["#10", "#5", "#2"], "asc")).toEqual(["#2", "#5", "#10"]);
    expect(sorted(["#10", "#5", "#2"], "desc")).toEqual(["#10", "#5", "#2"]);
  });

  it("compares strings case-insensitively", () => {
    expect(compareSortValues("alpha", "ALPHA", "asc")).toBe(0);
    expect(sorted(["beta", "Alpha"], "asc")).toEqual(["Alpha", "beta"]);
  });

  it("sinks nulls whatever the direction", () => {
    expect(sorted([null, "b", "a"], "asc")).toEqual(["a", "b", null]);
    expect(sorted([null, "b", "a"], "desc")).toEqual(["b", "a", null]);
  });
});
