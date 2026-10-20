// 원형 메뉴 기하: 각도 → 칸, 도넛 조각 path, 제스처 판정. 모두 순수 함수.
//
// 칸 번호 규약 (core `default_layout`과 동일):
//  - 각도 θ는 12시 = 0°, 시계 방향, [0, 360).
//  - 칸 i = [(270 + i*360/N) mod 360, +360/N), 시작 포함 끝 제외 (9시에서 시작해 시계 방향).
//  - 더 보기 칸 = θ=135°가 들어 있는 칸 (N=4 → 2, N=8 → 5).

export const OUTER_RADIUS = 120;
export const HUB_RADIUS = 48; // 지름 96
export const DEAD_ZONE = 40;
export const MORE_INNER = 128;
export const MORE_OUTER = 184;
export const GAP = 4;
export const GESTURE_CLICK_MS = 250;
export const CENTER_DEFAULT = 240;

export type Hit =
  | { kind: "none" } // 데드존
  | { kind: "outside" } // 메뉴 바깥
  | { kind: "main"; index: number }
  | { kind: "more"; index: number };

export type SlotHit = Extract<Hit, { kind: "main" | "more" }>;

export interface HitConfig {
  slotCount: number;
  moreCount: number;
  moreOpen: boolean;
}

/** 중심 기준 (dx, dy) (화면 좌표, y는 아래로 증가) → 12시 기준 시계 방향 각도 [0, 360) */
export function pointAngle(dx: number, dy: number): number {
  const deg = (Math.atan2(dx, -dy) * 180) / Math.PI;
  const a = ((Math.round(deg * 1e9) / 1e9 % 360) + 360) % 360;
  return a >= 360 ? 0 : a;
}

/** 각도 → 칸 번호. 시작 포함, 끝 제외. */
export function angleToSlot(angle: number, n: number): number {
  const a = ((angle % 360) + 360) % 360;
  const rel = (((a - 270) % 360) + 360) % 360;
  const index = Math.floor(rel / (360 / n) + 1e-9);
  return Math.min(index, n - 1);
}

/** 더 보기 칸 번호: θ=135°가 들어 있는 칸 */
export function moreSlotIndex(n: number): number {
  return angleToSlot(135, n);
}

/** 칸 i의 시작·끝 각도(끝은 start + 360/n, 360을 넘을 수 있음). 중앙각도 포함. */
export function slotRange(index: number, n: number): { start: number; end: number; mid: number } {
  const w = 360 / n;
  const start = 270 + index * w;
  return { start, end: start + w, mid: start + w / 2 };
}

export function distance(dx: number, dy: number): number {
  return Math.hypot(dx, dy);
}

export function outerRadius(moreOpen: boolean): number {
  return moreOpen ? MORE_OUTER : OUTER_RADIUS;
}

/**
 * 중심 기준 상대 좌표 → 칸.
 * - 거리 < 데드존: none
 * - 두 번째 고리가 열려 있고 거리 ≥ 128: 128~184면 more, 그 밖은 outside
 * - 그 밖: 바깥 반지름(고리 상태에 따라 120 또는 184) 이내면 main, 넘으면 outside
 */
export function hitTest(dx: number, dy: number, cfg: HitConfig): Hit {
  const d = distance(dx, dy);
  if (d < DEAD_ZONE) return { kind: "none" };
  const angle = pointAngle(dx, dy);
  if (cfg.moreOpen && cfg.moreCount > 0 && d >= MORE_INNER) {
    if (d > MORE_OUTER) return { kind: "outside" };
    return { kind: "more", index: angleToSlot(angle, cfg.moreCount) };
  }
  if (d > outerRadius(cfg.moreOpen && cfg.moreCount > 0)) return { kind: "outside" };
  return { kind: "main", index: angleToSlot(angle, cfg.slotCount) };
}

export type GestureDecision =
  | { kind: "select"; hit: SlotHit }
  | { kind: "keep_open" }
  | { kind: "cancel" };

export interface GestureInput {
  dx: number;
  dy: number;
  held_ms: number;
  /** 열린 뒤 포인터가 데드존 밖으로 한 번이라도 나갔는가 */
  leftDeadZone: boolean;
  config: HitConfig;
}

/**
 * 휠 버튼을 뗐을 때의 판정 (세션 계약 4).
 * - 데드존 밖에서 떼면 그 자리의 칸을 고른다 (메뉴 바깥이어도 각도로 바깥 고리 칸).
 * - 데드존 안에서 떼고, 데드존을 한 번도 벗어나지 않았고, held_ms < 250 이면 클릭 방식 → 열어 둔다.
 * - 그 밖에 데드존 안에서 떼면 취소.
 */
export function decideGesture(g: GestureInput): GestureDecision {
  const d = distance(g.dx, g.dy);
  if (d < DEAD_ZONE) {
    return !g.leftDeadZone && g.held_ms < GESTURE_CLICK_MS ? { kind: "keep_open" } : { kind: "cancel" };
  }
  const hit = hitTest(g.dx, g.dy, g.config);
  if (hit.kind === "main" || hit.kind === "more") return { kind: "select", hit };
  // 메뉴 바깥에서 떼도 방향으로 고른다 (끌기가 고리 밖까지 나가는 경우)
  return { kind: "select", hit: { kind: "main", index: angleToSlot(pointAngle(g.dx, g.dy), g.config.slotCount) } };
}

function polar(cx: number, cy: number, r: number, deg: number): [number, number] {
  const rad = (deg * Math.PI) / 180;
  return [cx + r * Math.sin(rad), cy - r * Math.cos(rad)];
}

const f = (v: number) => Number(v.toFixed(2));

/**
 * 도넛 조각 SVG path. 칸 사이 틈(gap px)은 각 반지름에서 시작·끝 각도를 줄여 만든다
 * (평행한 틈이 되도록 asin((gap/2)/r)). 선을 덧그리지 않는다.
 */
export function donutPath(
  cx: number,
  cy: number,
  rIn: number,
  rOut: number,
  startDeg: number,
  endDeg: number,
  gap: number = GAP,
): string {
  const trim = (r: number) => (Math.asin(Math.min(1, gap / 2 / r)) * 180) / Math.PI;
  const oS = startDeg + trim(rOut);
  const oE = endDeg - trim(rOut);
  const iS = startDeg + trim(rIn);
  const iE = endDeg - trim(rIn);
  const large = oE - oS > 180 ? 1 : 0;
  const [x1, y1] = polar(cx, cy, rOut, oS);
  const [x2, y2] = polar(cx, cy, rOut, oE);
  const [x3, y3] = polar(cx, cy, rIn, iE);
  const [x4, y4] = polar(cx, cy, rIn, iS);
  return [
    `M ${f(x1)} ${f(y1)}`,
    `A ${rOut} ${rOut} 0 ${large} 1 ${f(x2)} ${f(y2)}`,
    `L ${f(x3)} ${f(y3)}`,
    `A ${rIn} ${rIn} 0 ${large} 0 ${f(x4)} ${f(y4)}`,
    "Z",
  ].join(" ");
}

/** 칸 중심 좌표 (라벨 위치) */
export function labelPoint(cx: number, cy: number, r: number, midDeg: number): { x: number; y: number } {
  const [x, y] = polar(cx, cy, r, midDeg);
  return { x, y };
}

/** 칸의 호 길이(px) at 반지름 r. 설명 글자를 넣을 수 있는지 판단하는 데 쓴다. */
export function arcLength(r: number, n: number): number {
  return (2 * Math.PI * r) / n;
}
