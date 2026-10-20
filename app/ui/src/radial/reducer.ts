// 원형 메뉴 상태: 순수 reducer. 부수 효과(IPC 호출, 타이머)는 컴포넌트가 phase를 보고 처리한다.
import { baseName } from "../shared/path";
import type {
  FormatOption,
  FormatsReport,
  MButtonUpPayload,
  RadialClosePayload,
  RadialOpenPayload,
  ThemeInfo,
} from "../shared/ipc";
import { DEAD_ZONE, decideGesture, distance, hitTest, type Hit, type HitConfig } from "./geometry";

export type Ring = "main" | "more";
export interface Target {
  ring: Ring;
  index: number;
}

export type Phase = "closed" | "open" | "choosing" | "canceling";

export interface RadialState {
  phase: Phase;
  session: RadialOpenPayload | null;
  /** 세션과 무관한 마지막 테마 (닫아도 유지) */
  theme: ThemeInfo | null;
  /** 커서 또는 키보드가 가리키는 칸 */
  active: Target | null;
  moreOpen: boolean;
  /** 열린 뒤 포인터가 데드존 밖으로 나간 적이 있는가 (제스처 판정용) */
  leftDeadZone: boolean;
  chosen: { format_id: string; target: Target } | null;
  /** aria-live 영역에 쓸 낭독 문구 */
  announcement: string;
}

export const initialState: RadialState = {
  phase: "closed",
  session: null,
  theme: null,
  active: null,
  moreOpen: false,
  leftDeadZone: false,
  chosen: null,
  announcement: "",
};

export type RadialAction =
  | { type: "open"; payload: RadialOpenPayload }
  | { type: "remote_close"; payload: RadialClosePayload }
  | { type: "theme"; theme: ThemeInfo }
  | { type: "pointer_move"; x: number; y: number }
  | { type: "press"; x: number; y: number }
  | { type: "gesture_release"; payload: MButtonUpPayload }
  | { type: "move"; target: Target }
  | { type: "activate"; target: Target }
  | { type: "cancel" }
  | { type: "finish"; session_id: number };

export type Item =
  | { kind: "format"; option: FormatOption; position: number; total: number }
  | { kind: "more"; position: number; total: number }
  | { kind: "empty" };

export function ringSize(session: RadialOpenPayload, ring: Ring): number {
  const l = session.report.layout;
  return ring === "main" ? l.slot_count : l.more.length;
}

export function itemAt(session: RadialOpenPayload, target: Target): Item {
  const { layout, options } = session.report;
  const total = ringSize(session, target.ring);
  const position = target.index + 1;
  if (target.ring === "more") {
    const id = layout.more[target.index];
    const option = options.find((o) => o.id === id);
    return option ? { kind: "format", option, position, total } : { kind: "empty" };
  }
  const slot = layout.slots[target.index];
  if (!slot || slot.kind === "empty") return { kind: "empty" };
  if (slot.kind === "more") return { kind: "more", position, total };
  const option = options.find((o) => o.id === slot.format_id);
  return option ? { kind: "format", option, position, total } : { kind: "empty" };
}

export function hitConfig(state: Pick<RadialState, "session" | "moreOpen">): HitConfig {
  const l = state.session?.report.layout;
  return { slotCount: l?.slot_count ?? 4, moreCount: l?.more.length ?? 0, moreOpen: state.moreOpen };
}

export function sameTarget(a: Target | null, b: Target | null): boolean {
  return a === b || (a !== null && b !== null && a.ring === b.ring && a.index === b.index);
}

export function hubText(report: FormatsReport): string {
  const n = report.files.length;
  if (n === 1) return (report.files[0]?.format ?? "").toUpperCase();
  return `${n}개 파일`;
}

function fileDescription(report: FormatsReport): string {
  const n = report.files.length;
  if (n === 1) return baseName(report.files[0]?.path ?? "");
  return `${n}개 파일`;
}

function describeItem(item: Item): string {
  if (item.kind === "empty") return "";
  if (item.kind === "more") return `더 보기 ${item.position}/${item.total}`;
  const base = `${item.option.label} ${item.position}/${item.total}`;
  return item.option.available
    ? base
    : `${base}, 사용할 수 없음: ${item.option.unavailable?.message ?? ""}`;
}

/** 열릴 때 낭독: "변환 메뉴, 발표녹화.mov, MP4 1/4" */
export function openingAnnouncement(session: RadialOpenPayload): string {
  const first = session.report.layout.slots.findIndex((s) => s.kind !== "empty");
  const item = first < 0 ? null : itemAt(session, { ring: "main", index: first });
  const head = `변환 메뉴, ${fileDescription(session.report)}`;
  return item && item.kind !== "empty" ? `${head}, ${describeItem(item)}` : head;
}

/** 안내 줄 문구: 기본 = 파일 이름, 칸 위 = 형식 설명, 사용 불가 칸 = 이유 */
export function guideText(state: RadialState): string {
  const s = state.session;
  if (!s) return "";
  if (state.active) {
    const item = itemAt(s, state.active);
    if (item.kind === "format") {
      return item.option.available
        ? item.option.description
        : (item.option.unavailable?.message ?? "사용할 수 없습니다");
    }
    if (item.kind === "more") return "더 보기";
  }
  const n = s.report.files.length;
  if (n === 1) return baseName(s.report.files[0]?.path ?? "");
  const first = baseName(s.report.files[0]?.path ?? "");
  return `${first} 외 ${n - 1}개`;
}

function withActive(state: RadialState, target: Target | null): RadialState {
  if (sameTarget(state.active, target)) return state;
  const session = state.session;
  const item = session && target ? itemAt(session, target) : null;
  return {
    ...state,
    active: item && item.kind !== "empty" ? target : null,
    announcement: item && item.kind !== "empty" ? describeItem(item) : state.announcement,
  };
}

function hitToTarget(hit: Hit): Target | null {
  return hit.kind === "main" || hit.kind === "more" ? { ring: hit.kind, index: hit.index } : null;
}

function reset(state: RadialState): RadialState {
  return { ...initialState, theme: state.theme };
}

function activate(state: RadialState, target: Target): RadialState {
  const session = state.session;
  if (state.phase !== "open" || !session) return state;
  const item = itemAt(session, target);
  if (item.kind === "empty") return state;
  const focused = withActive(state, target);
  if (item.kind === "more") {
    if (session.report.layout.more.length === 0) return focused;
    const moreOpen = !state.moreOpen;
    return {
      ...focused,
      moreOpen,
      announcement: moreOpen
        ? `더 보기, 항목 ${session.report.layout.more.length}개`
        : "더 보기를 닫았습니다",
    };
  }
  if (!item.option.available) {
    return { ...focused, announcement: describeItem(item) };
  }
  return {
    ...focused,
    phase: "choosing",
    chosen: { format_id: item.option.id, target },
    announcement: `${item.option.label} 변환을 시작합니다`,
  };
}

function cancel(state: RadialState): RadialState {
  if (state.phase !== "open") return state;
  return { ...state, phase: "canceling", announcement: "취소했습니다" };
}

export function reducer(state: RadialState, action: RadialAction): RadialState {
  switch (action.type) {
    case "open": {
      const p = action.payload;
      return {
        ...initialState,
        phase: "open",
        session: p,
        theme: p.theme,
        announcement: openingAnnouncement(p),
      };
    }
    case "remote_close": {
      // 오래된 세션의 close는 무시한다.
      if (state.session && action.payload.session_id !== state.session.session_id) return state;
      return reset(state);
    }
    case "finish":
      // 이미 새 세션이 열렸으면(다른 session_id) 건드리지 않는다.
      if (state.session && action.session_id !== state.session.session_id) return state;
      return reset(state);
    case "theme":
      return { ...state, theme: action.theme };
    case "pointer_move": {
      const s = state.session;
      if (state.phase !== "open" || !s) return state;
      const dx = action.x - s.center.x;
      const dy = action.y - s.center.y;
      const left = state.leftDeadZone || distance(dx, dy) >= DEAD_ZONE;
      const next = withActive(state, hitToTarget(hitTest(dx, dy, hitConfig(state))));
      return left === next.leftDeadZone ? next : { ...next, leftDeadZone: left };
    }
    case "press": {
      const s = state.session;
      if (state.phase !== "open" || !s) return state;
      const hit = hitTest(action.x - s.center.x, action.y - s.center.y, hitConfig(state));
      if (hit.kind === "outside") return cancel(state);
      const target = hitToTarget(hit);
      return target ? activate(state, target) : state;
    }
    case "gesture_release": {
      const s = state.session;
      const p = action.payload;
      if (state.phase !== "open" || !s || p.session_id !== s.session_id) return state;
      const decision = decideGesture({
        dx: p.x - s.center.x,
        dy: p.y - s.center.y,
        held_ms: p.held_ms,
        leftDeadZone: state.leftDeadZone,
        config: hitConfig(state),
      });
      if (decision.kind === "cancel") return cancel(state);
      if (decision.kind === "keep_open") return state;
      return activate(state, { ring: decision.hit.kind, index: decision.hit.index });
    }
    case "move":
      return state.phase === "open" ? withActive(state, action.target) : state;
    case "activate":
      return activate(state, action.target);
    case "cancel":
      return cancel(state);
  }
}
