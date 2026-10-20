import { describe, expect, it } from "vitest";
import type { RadialSlot } from "../shared/ipc";
import { mapKey, type KeyContext } from "./keyboard";

const f = (id: string): RadialSlot => ({ kind: "format", format_id: id });
const slots4: RadialSlot[] = [f("mp4"), f("mp3"), { kind: "more" }, f("gif")];
const ctx4 = (over: Partial<KeyContext> = {}): KeyContext => ({
  slots: slots4,
  moreCount: 2,
  moreOpen: false,
  active: null,
  ...over,
});
const at = (index: number) => ({ ring: "main" as const, index });

describe("mapKey", () => {
  it("Esc = 취소, Enter = 현재 칸 고르기 (칸 없으면 무시)", () => {
    expect(mapKey("Escape", ctx4())).toEqual({ type: "cancel" });
    expect(mapKey("Enter", ctx4())).toEqual({ type: "none" });
    expect(mapKey("Enter", ctx4({ active: at(1) }))).toEqual({ type: "activate", target: at(1) });
  });

  it("숫자 k = 칸 k-1", () => {
    expect(mapKey("1", ctx4())).toEqual({ type: "activate", target: at(0) });
    expect(mapKey("3", ctx4())).toEqual({ type: "activate", target: at(2) });
    expect(mapKey("4", ctx4())).toEqual({ type: "activate", target: at(3) });
    expect(mapKey("5", ctx4())).toEqual({ type: "none" });
    expect(mapKey("0", ctx4())).toEqual({ type: "none" });
    expect(mapKey("9", ctx4())).toEqual({ type: "none" });
  });

  it("두 번째 고리가 열려 있으면 숫자는 그 고리의 항목", () => {
    expect(mapKey("2", ctx4({ moreOpen: true }))).toEqual({ type: "activate", target: { ring: "more", index: 1 } });
    expect(mapKey("3", ctx4({ moreOpen: true }))).toEqual({ type: "none" });
  });

  it("빈 칸 숫자는 무시", () => {
    const slots = [f("a"), f("b"), { kind: "empty" } as RadialSlot, f("c")];
    expect(mapKey("3", ctx4({ slots }))).toEqual({ type: "none" });
  });

  it("N=4 방향키는 공간 이동 (좌우 같은 줄, 상하 같은 열)", () => {
    expect(mapKey("ArrowRight", ctx4({ active: at(0) }))).toEqual({ type: "move", target: at(1) });
    expect(mapKey("ArrowLeft", ctx4({ active: at(1) }))).toEqual({ type: "move", target: at(0) });
    expect(mapKey("ArrowRight", ctx4({ active: at(3) }))).toEqual({ type: "move", target: at(2) });
    expect(mapKey("ArrowDown", ctx4({ active: at(0) }))).toEqual({ type: "move", target: at(3) });
    expect(mapKey("ArrowUp", ctx4({ active: at(3) }))).toEqual({ type: "move", target: at(0) });
    expect(mapKey("ArrowDown", ctx4({ active: at(1) }))).toEqual({ type: "move", target: at(2) });
    expect(mapKey("ArrowUp", ctx4({ active: at(2) }))).toEqual({ type: "move", target: at(1) });
  });

  it("N=4 가장자리에서는 그대로", () => {
    expect(mapKey("ArrowLeft", ctx4({ active: at(0) }))).toEqual({ type: "none" });
    expect(mapKey("ArrowUp", ctx4({ active: at(1) }))).toEqual({ type: "none" });
  });

  it("N=4에서 현재 칸이 없을 때 첫 방향키는 순환 규칙으로 시작", () => {
    expect(mapKey("ArrowRight", ctx4())).toEqual({ type: "move", target: at(0) });
    expect(mapKey("ArrowLeft", ctx4())).toEqual({ type: "move", target: at(3) });
  });

  it("N=8: 오른쪽·아래 시계 방향, 왼쪽·위 반시계 방향, 순환", () => {
    const slots8: RadialSlot[] = Array.from({ length: 8 }, (_, i) => f(`f${i}`));
    const c = (active: number): KeyContext => ({ slots: slots8, moreCount: 0, moreOpen: false, active: at(active) });
    expect(mapKey("ArrowRight", c(2))).toEqual({ type: "move", target: at(3) });
    expect(mapKey("ArrowDown", c(7))).toEqual({ type: "move", target: at(0) });
    expect(mapKey("ArrowLeft", c(0))).toEqual({ type: "move", target: at(7) });
    expect(mapKey("ArrowUp", c(5))).toEqual({ type: "move", target: at(4) });
  });

  it("N≠4: 빈 칸은 건너뛴다", () => {
    const slots: RadialSlot[] = [f("a"), { kind: "empty" }, f("c"), f("d"), f("e"), f("g")];
    const c: KeyContext = { slots, moreCount: 0, moreOpen: false, active: at(0) };
    expect(mapKey("ArrowRight", c)).toEqual({ type: "move", target: at(2) });
  });

  it("두 번째 고리 안에서는 순환 이동", () => {
    const c: KeyContext = { slots: slots4, moreCount: 3, moreOpen: true, active: { ring: "more", index: 2 } };
    expect(mapKey("ArrowRight", c)).toEqual({ type: "move", target: { ring: "more", index: 0 } });
    expect(mapKey("ArrowLeft", c)).toEqual({ type: "move", target: { ring: "more", index: 1 } });
  });

  it("그 밖의 키는 무시", () => {
    expect(mapKey("a", ctx4())).toEqual({ type: "none" });
    expect(mapKey("Tab", ctx4())).toEqual({ type: "none" });
  });
});
