// 키 입력 → 동작 매핑. 순수 함수.
//  - 숫자 1~8 = 해당 칸 (두 번째 고리가 열려 있으면 그 고리의 항목), 방향키 = 칸 이동,
//    Enter = 고르기, Esc = 취소.
//  - N=4: 공간 이동 (←→ 같은 줄, ↑↓ 같은 열). N≠4와 두 번째 고리: →↓ 시계 방향, ←↑ 반시계 방향.
import type { RadialSlot } from "../shared/ipc";
import type { Target } from "./reducer";

export type KeyCommand =
  | { type: "move"; target: Target }
  | { type: "activate"; target: Target }
  | { type: "cancel" }
  | { type: "none" };

export interface KeyContext {
  slots: RadialSlot[];
  moreCount: number;
  moreOpen: boolean;
  active: Target | null;
}

const NONE: KeyCommand = { type: "none" };

// N=4 공간 이동표: 0=왼쪽 위, 1=오른쪽 위, 2=오른쪽 아래, 3=왼쪽 아래
const SPATIAL4: Record<string, Record<number, number>> = {
  ArrowLeft: { 1: 0, 2: 3 },
  ArrowRight: { 0: 1, 3: 2 },
  ArrowUp: { 3: 0, 2: 1 },
  ArrowDown: { 0: 3, 1: 2 },
};

const CLOCKWISE = new Set(["ArrowRight", "ArrowDown"]);
const COUNTER = new Set(["ArrowLeft", "ArrowUp"]);

/** 순환 이동. 비어 있는 칸은 건너뛴다. 가리킬 칸이 없으면 null. */
function cycle(from: number | null, dir: 1 | -1, count: number, usable: (i: number) => boolean): number | null {
  if (count <= 0) return null;
  const start = from ?? (dir === 1 ? -1 : 0);
  for (let step = 1; step <= count; step++) {
    const i = (((start + dir * step) % count) + count) % count;
    if (usable(i)) return i;
  }
  return null;
}

export function mapKey(key: string, ctx: KeyContext): KeyCommand {
  const ring = ctx.moreOpen && ctx.moreCount > 0 ? "more" : "main";
  const count = ring === "more" ? ctx.moreCount : ctx.slots.length;
  const usable = (i: number) => ring === "more" || ctx.slots[i]?.kind !== "empty";

  if (key === "Escape") return { type: "cancel" };

  if (key === "Enter") {
    return ctx.active ? { type: "activate", target: ctx.active } : NONE;
  }

  if (/^[1-8]$/.test(key)) {
    const index = Number(key) - 1;
    return index < count && usable(index) ? { type: "activate", target: { ring, index } } : NONE;
  }

  const clockwise = CLOCKWISE.has(key);
  if (!clockwise && !COUNTER.has(key)) return NONE;

  // 현재 칸이 다른 고리에 있으면 현재 고리의 처음부터 시작한다.
  const from = ctx.active && ctx.active.ring === ring ? ctx.active.index : null;

  if (ring === "main" && ctx.slots.length === 4 && from !== null) {
    const to = SPATIAL4[key]?.[from];
    if (to === undefined) return NONE; // 가장자리: 그대로
    return usable(to) ? { type: "move", target: { ring, index: to } } : NONE;
  }

  const index = cycle(from, clockwise ? 1 : -1, count, usable);
  return index === null ? NONE : { type: "move", target: { ring, index } };
}
