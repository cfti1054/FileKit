import { describe, expect, it } from "vitest";
import { mockLayout } from "../shared/mock";
import { resolveMotion } from "./motion";

describe("resolveMotion", () => {
  it("둘 다 허용이면 켠다", () => expect(resolveMotion(true, false)).toBe(true));
  it("Rust가 끄면 끈다", () => expect(resolveMotion(false, false)).toBe(false));
  it("Rust가 켜도 reduced-motion이면 끈다", () => expect(resolveMotion(true, true)).toBe(false));
  it("둘 다 끔이면 끈다", () => expect(resolveMotion(false, true)).toBe(false));
  it("theme 없으면 reduced-motion만 따른다", () => {
    expect(resolveMotion(undefined, false)).toBe(true);
    expect(resolveMotion(undefined, true)).toBe(false);
  });
});

describe("mockLayout 더 보기 규칙(N=4)", () => {
  it("3개면 더 보기 없이 2번 칸이 비어 있다", () => {
    const l = mockLayout(["a", "b", "c"], 4);
    expect(l.more).toEqual([]);
    expect(l.slots[2]).toEqual({ kind: "empty" });
    expect(l.slots.filter((s) => s.kind === "format")).toHaveLength(3);
  });
  it("4개면 3개 + 2번 칸 더 보기", () => {
    const l = mockLayout(["a", "b", "c", "d"], 4);
    expect(l.slots[2]).toEqual({ kind: "more" });
    expect(l.more).toEqual(["d"]);
  });
});
