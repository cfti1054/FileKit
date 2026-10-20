import { describe, expect, it } from "vitest";
import {
  angleToSlot,
  decideGesture,
  donutPath,
  hitTest,
  moreSlotIndex,
  pointAngle,
  slotRange,
  type HitConfig,
} from "./geometry";

const cfg4: HitConfig = { slotCount: 4, moreCount: 0, moreOpen: false };

describe("pointAngle (12시 = 0°, 시계 방향)", () => {
  it("네 방향", () => {
    expect(pointAngle(0, -10)).toBe(0); // 위
    expect(pointAngle(10, 0)).toBe(90); // 오른쪽
    expect(pointAngle(0, 10)).toBe(180); // 아래
    expect(pointAngle(-10, 0)).toBe(270); // 왼쪽
  });
  it("대각선", () => {
    expect(pointAngle(10, -10)).toBeCloseTo(45, 9);
    expect(pointAngle(10, 10)).toBeCloseTo(135, 9);
    expect(pointAngle(-10, -10)).toBeCloseTo(315, 9);
  });
});

describe("angleToSlot: 9시 방향에서 시작해 시계 방향, 시작 포함 끝 제외", () => {
  it("N=4 경계", () => {
    expect(angleToSlot(270, 4)).toBe(0);
    expect(angleToSlot(359.99, 4)).toBe(0);
    expect(angleToSlot(0, 4)).toBe(1);
    expect(angleToSlot(89.99, 4)).toBe(1);
    expect(angleToSlot(90, 4)).toBe(2);
    expect(angleToSlot(180, 4)).toBe(3);
    expect(angleToSlot(269.99, 4)).toBe(3);
  });
  it("N=4 대각선: 왼쪽 위=0, 오른쪽 위=1, 오른쪽 아래=2, 왼쪽 아래=3", () => {
    expect(angleToSlot(315, 4)).toBe(0);
    expect(angleToSlot(45, 4)).toBe(1);
    expect(angleToSlot(135, 4)).toBe(2);
    expect(angleToSlot(225, 4)).toBe(3);
  });
  it("N=8 경계 (칸 폭 45°)", () => {
    expect(angleToSlot(270, 8)).toBe(0);
    expect(angleToSlot(314.99, 8)).toBe(0);
    expect(angleToSlot(315, 8)).toBe(1);
    expect(angleToSlot(0, 8)).toBe(2);
    expect(angleToSlot(135, 8)).toBe(5);
    expect(angleToSlot(134.99, 8)).toBe(4);
    expect(angleToSlot(225, 8)).toBe(7);
  });
  it("범위 밖 각도는 정규화", () => {
    expect(angleToSlot(-90, 4)).toBe(0);
    expect(angleToSlot(360, 4)).toBe(1);
    expect(angleToSlot(630, 4)).toBe(0);
  });
  it("그 밖의 N에서도 모든 칸이 한 번씩 나온다", () => {
    for (const n of [3, 5, 6, 7]) {
      const seen = new Set<number>();
      for (let a = 0; a < 360; a += 0.5) seen.add(angleToSlot(a, n));
      expect(seen.size).toBe(n);
    }
  });
});

describe("더 보기 칸", () => {
  it("θ=135°가 들어 있는 칸: N=4 → 2, N=8 → 5", () => {
    expect(moreSlotIndex(4)).toBe(2);
    expect(moreSlotIndex(8)).toBe(5);
  });
  it("slotRange", () => {
    expect(slotRange(0, 4)).toEqual({ start: 270, end: 360, mid: 315 });
    expect(slotRange(2, 4).mid % 360).toBe(135); // 12시 기준 각도 (그림 그리기용으로 360을 넘을 수 있다)
    expect(slotRange(5, 8).mid % 360).toBe(157.5);
  });
});

describe("hitTest", () => {
  it("데드존 경계: 반지름 40 미만은 none, 40부터 칸", () => {
    expect(hitTest(0, 0, cfg4)).toEqual({ kind: "none" });
    expect(hitTest(39.99, 0, cfg4)).toEqual({ kind: "none" });
    expect(hitTest(40, 0, cfg4)).toEqual({ kind: "main", index: 2 }); // 오른쪽 90° → 칸 2
    expect(hitTest(0, -40, cfg4)).toEqual({ kind: "main", index: 1 });
  });
  it("바깥 반지름 120을 넘으면 outside", () => {
    expect(hitTest(120, 0, cfg4)).toEqual({ kind: "main", index: 2 });
    expect(hitTest(120.5, 0, cfg4)).toEqual({ kind: "outside" });
  });
  it("두 번째 고리: 128~184에서 more, 안쪽은 main", () => {
    const c: HitConfig = { slotCount: 4, moreCount: 3, moreOpen: true };
    expect(hitTest(100, 0, c)).toEqual({ kind: "main", index: 2 });
    expect(hitTest(127, 0, c)).toEqual({ kind: "main", index: 2 });
    expect(hitTest(128, 0, c)).toEqual({ kind: "more", index: 1 }); // 90° → ((90-270+360)%360)/120 = 1.5 → 1
    expect(hitTest(184, 0, c)).toEqual({ kind: "more", index: 1 });
    expect(hitTest(184.5, 0, c)).toEqual({ kind: "outside" });
  });
  it("두 번째 고리가 닫혀 있으면 128 이상은 outside", () => {
    const c: HitConfig = { slotCount: 4, moreCount: 3, moreOpen: false };
    expect(hitTest(130, 0, c)).toEqual({ kind: "outside" });
  });
});

describe("decideGesture (휠 버튼 놓기)", () => {
  const base = { held_ms: 400, leftDeadZone: true, config: cfg4 };
  it("데드존 밖에서 놓으면 그 자리의 칸을 고른다", () => {
    expect(decideGesture({ ...base, dx: 80, dy: -80 })).toEqual({ kind: "select", hit: { kind: "main", index: 1 } });
  });
  it("메뉴 바깥(반지름 300)에서 놓아도 방향으로 칸을 고른다", () => {
    expect(decideGesture({ ...base, dx: -300, dy: -300 })).toEqual({ kind: "select", hit: { kind: "main", index: 0 } });
  });
  it("데드존에서 놓으면 취소 (끌었다 돌아온 경우)", () => {
    expect(decideGesture({ ...base, dx: 5, dy: 5 })).toEqual({ kind: "cancel" });
  });
  it("데드존을 한 번도 벗어나지 않고 250ms 미만이면 클릭 방식: 열어 둔다", () => {
    expect(decideGesture({ dx: 0, dy: 0, held_ms: 249, leftDeadZone: false, config: cfg4 })).toEqual({ kind: "keep_open" });
  });
  it("250ms 이상이면 데드존에서 취소", () => {
    expect(decideGesture({ dx: 0, dy: 0, held_ms: 250, leftDeadZone: false, config: cfg4 })).toEqual({ kind: "cancel" });
  });
  it("벗어난 적이 있으면 짧아도 취소", () => {
    expect(decideGesture({ dx: 0, dy: 0, held_ms: 100, leftDeadZone: true, config: cfg4 })).toEqual({ kind: "cancel" });
  });
});

describe("donutPath", () => {
  it("닫힌 path를 만든다", () => {
    const d = donutPath(240, 240, 48, 120, 270, 360);
    expect(d.startsWith("M ")).toBe(true);
    expect(d.endsWith("Z")).toBe(true);
    expect(d.match(/A /g)).toHaveLength(2);
    expect(d).not.toContain("NaN");
  });
  it("180° 넘는 조각은 large-arc 플래그", () => {
    expect(donutPath(240, 240, 128, 184, 270, 630)).toContain("0 1 1");
  });
});
