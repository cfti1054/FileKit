import { useEffect, useMemo, useReducer, useRef, useState, type CSSProperties, type PointerEvent } from "react";
import { resolveAccent } from "../shared/contrast";
import { resolveMotion } from "./motion";
import type { Ipc, RadialOpenPayload } from "../shared/ipc";
import {
  arcLength,
  donutPath,
  HUB_RADIUS,
  labelPoint,
  moreSlotIndex,
  MORE_INNER,
  MORE_OUTER,
  OUTER_RADIUS,
  slotRange,
} from "./geometry";
import { mapKey } from "./keyboard";
import {
  guideText,
  hubText,
  initialState,
  itemAt,
  reducer,
  sameTarget,
  type RadialState,
  type Ring,
  type Target,
} from "./reducer";
import "./radial.css";

/** 창 논리 크기. 메뉴 중심은 (240, 240). */
const STAGE = 480;
const GUIDE_HEIGHT = 28;
const GUIDE_GAP = 12;
/** 이 호 길이(px) 미만이면 설명 줄을 생략한다 (칸이 좁아 글자가 넘침) */
const MIN_ARC_FOR_DESCRIPTION = 96;

function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(query).matches
      : false,
  );
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const mq = window.matchMedia(query);
    const on = () => setMatches(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, [query]);
  return matches;
}

interface Props {
  ipc: Ipc;
  /** mock 확인용: 열리자마자 두 번째 고리를 펼친다 */
  autoOpenMore?: boolean;
}

/** 컨테이너: IPC 이벤트 구독, reducer, 타이머, 입력 연결 */
export function RadialMenu({ ipc, autoOpenMore = false }: Props) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const stageRef = useRef<HTMLDivElement>(null);
  const prefersReduced = useMediaQuery("(prefers-reduced-motion: reduce)");
  const dark = useMediaQuery("(prefers-color-scheme: dark)");
  // Rust 설정과 브라우저 reduced-motion 중 하나라도 끔이면 끈다.
  const motion = resolveMotion(state.theme?.animations_enabled, prefersReduced);

  // 이벤트 구독 → 준비됐다고 알림(radial_ready) → 먼저 열린 메뉴가 있으면 처리
  useEffect(() => {
    let disposed = false;
    const unsubs: Array<() => void> = [];
    const open = (payload: RadialOpenPayload) => {
      dispatch({ type: "open", payload });
      if (autoOpenMore) {
        dispatch({ type: "activate", target: { ring: "main", index: moreSlotIndex(payload.report.layout.slot_count) } });
      }
    };
    void Promise.all([
      ipc.on("radial:open", open),
      ipc.on("radial:close", (payload) => dispatch({ type: "remote_close", payload })),
      ipc.on("radial:mbutton_up", (payload) => dispatch({ type: "gesture_release", payload })),
      ipc.on("theme:changed", (theme) => dispatch({ type: "theme", theme })),
    ]).then((us) => {
      if (disposed) {
        us.forEach((u) => u());
        return;
      }
      unsubs.push(...us);
      ipc
        .radialReady()
        .then((p) => {
          if (p && !disposed) open(p);
        })
        .catch(() => undefined);
    });
    return () => {
      disposed = true;
      unsubs.forEach((u) => u());
    };
  }, [ipc, autoOpenMore]);

  // 고르기(120ms)·취소(100ms) 애니메이션이 끝난 뒤 Rust에 알린다. 움직임이 꺼져 있으면 0ms.
  useEffect(() => {
    const session = state.session;
    if (!session) return;
    if (state.phase !== "choosing" && state.phase !== "canceling") return;
    const chosen = state.chosen;
    const ms = motion ? (state.phase === "choosing" ? 120 : 100) : 0;
    const t = window.setTimeout(() => {
      const call = state.phase === "choosing" && chosen
        ? ipc.radialSelect(session.session_id, chosen.format_id)
        : ipc.radialCancel(session.session_id);
      call.catch(() => undefined).finally(() => dispatch({ type: "finish", session_id: session.session_id }));
    }, ms);
    return () => window.clearTimeout(t);
  }, [state.phase, state.session, state.chosen, motion, ipc]);

  // 키보드: 오버레이 창이 포커스를 받으므로 window에서 받는다 (키보드 훅 아님)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (state.phase !== "open" || !state.session) return;
      const l = state.session.report.layout;
      const cmd = mapKey(e.key, {
        slots: l.slots,
        moreCount: l.more.length,
        moreOpen: state.moreOpen,
        active: state.active,
      });
      if (cmd.type === "none") return;
      e.preventDefault();
      if (cmd.type === "cancel") dispatch({ type: "cancel" });
      else dispatch({ type: cmd.type, target: cmd.target });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [state.phase, state.session, state.moreOpen, state.active]);

  const toStage = (e: PointerEvent): { x: number; y: number } | null => {
    const r = stageRef.current?.getBoundingClientRect();
    if (!r || r.width === 0) return null;
    return { x: ((e.clientX - r.left) * STAGE) / r.width, y: ((e.clientY - r.top) * STAGE) / r.width };
  };

  const accent = resolveAccent(state.theme?.accent ?? null, dark);
  const style = { "--accent": accent } as CSSProperties;

  return (
    <div
      className={rootClass(state, motion)}
      style={style}
      onPointerMove={(e) => {
        const p = toStage(e);
        if (p) dispatch({ type: "pointer_move", ...p });
      }}
      onPointerDown={(e) => {
        // 왼쪽(0) 또는 휠(1) 버튼으로 고른다. 바깥 클릭은 reducer가 취소로 처리한다.
        if (e.button !== 0 && e.button !== 1) return;
        const p = toStage(e);
        if (p) dispatch({ type: "press", ...p });
      }}
      onMouseDown={(e) => {
        if (e.button === 1) e.preventDefault(); // 휠 클릭 자동 스크롤 방지
      }}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div className="sr-only" aria-live="assertive" aria-atomic="true">
        {state.announcement}
      </div>
      <div ref={stageRef} className="stage" role="menu" aria-label="변환 메뉴" style={stageStyle(state)}>
        {state.session && <MenuView state={state} session={state.session} />}
      </div>
    </div>
  );
}

function rootClass(state: RadialState, motion: boolean): string {
  const phase = { closed: "is-closed", open: "is-open", choosing: "is-choosing", canceling: "is-canceling" }[state.phase];
  return ["radial-root", phase, motion ? "" : "no-motion", state.theme?.high_contrast ? "high-contrast" : ""]
    .filter(Boolean)
    .join(" ");
}

/** 고른 뒤 메뉴가 커서 쪽으로 줄어드는 방향 */
function stageStyle(state: RadialState): CSSProperties | undefined {
  const s = state.session;
  if (!s || !state.chosen) return undefined;
  const t = state.chosen.target;
  const n = t.ring === "main" ? s.report.layout.slot_count : s.report.layout.more.length;
  const rad = (slotRange(t.index, n).mid * Math.PI) / 180;
  return { "--tx": `${(Math.sin(rad) * 24).toFixed(1)}px`, "--ty": `${(-Math.cos(rad) * 24).toFixed(1)}px` } as CSSProperties;
}

interface RingProps {
  state: RadialState;
  session: RadialOpenPayload;
  ring: Ring;
  rIn: number;
  rOut: number;
}

function RingView({ state, session, ring, rIn, rOut }: RingProps) {
  const { center } = session;
  const n = ring === "main" ? session.report.layout.slot_count : session.report.layout.more.length;
  const mid = (rIn + rOut) / 2;
  const showDescription = arcLength(mid, n) >= MIN_ARC_FOR_DESCRIPTION;
  return (
    <>
      {Array.from({ length: n }, (_, index) => {
        const target: Target = { ring, index };
        const item = itemAt(session, target);
        const { start, end, mid: midDeg } = slotRange(index, n);
        const d = donutPath(center.x, center.y, rIn, rOut, start, end);
        if (item.kind === "empty") return <path key={index} d={d} className="slot is-empty" aria-hidden="true" />;
        const active = sameTarget(state.active, target);
        const chosen = sameTarget(state.chosen?.target ?? null, target);
        const disabled = item.kind === "format" && !item.option.available;
        const isMore = item.kind === "more";
        const label = item.kind === "format" ? item.option.label : "더 보기";
        const desc = item.kind === "format" ? item.option.description : `${session.report.layout.more.length}개 더`;
        const p = labelPoint(center.x, center.y, mid, midDeg);
        const cls = ["slot", active ? "is-active" : "", disabled ? "is-disabled" : "", chosen ? "is-chosen" : "",
          isMore && state.moreOpen ? "is-expanded" : ""].filter(Boolean).join(" ");
        const aria =
          item.kind === "format"
            ? `${label}, ${desc}${disabled ? `, 사용할 수 없음: ${item.option.unavailable?.message ?? ""}` : ""}`
            : `더 보기, ${desc}`;
        return (
          <g
            key={index}
            className={cls}
            role="menuitem"
            aria-label={aria}
            aria-disabled={disabled ? "true" : undefined}
            aria-haspopup={isMore ? "menu" : undefined}
            aria-expanded={isMore ? state.moreOpen : undefined}
          >
            <path d={d} className="shape" />
            <text className="label" x={p.x} y={p.y + (showDescription ? -2 : 6)} textAnchor="middle">
              {label}
            </text>
            {showDescription && (
              <text className="desc" x={p.x} y={p.y + 15} textAnchor="middle">
                {desc}
              </text>
            )}
          </g>
        );
      })}
    </>
  );
}

function MenuView({ state, session }: { state: RadialState; session: RadialOpenPayload }) {
  const { center, report } = session;
  const moreVisible = state.moreOpen && report.layout.more.length > 0;
  const outer = moreVisible ? MORE_OUTER : OUTER_RADIUS;
  const guideTop =
    session.guide_position === "above"
      ? center.y - outer - GUIDE_GAP - GUIDE_HEIGHT
      : center.y + outer + GUIDE_GAP;
  const text = useMemo(() => guideText(state), [state]);
  return (
    <>
      <svg className="menu" width={STAGE} height={STAGE} viewBox={`0 0 ${STAGE} ${STAGE}`} role="presentation">
        <RingView state={state} session={session} ring="main" rIn={HUB_RADIUS} rOut={OUTER_RADIUS} />
        {moreVisible && (
          <g className="more-ring">
            <RingView state={state} session={session} ring="more" rIn={MORE_INNER} rOut={MORE_OUTER} />
          </g>
        )}
        <g className="hub" aria-hidden="true">
          <circle cx={center.x} cy={center.y} r={HUB_RADIUS} />
          <text x={center.x} y={center.y + 6} textAnchor="middle">
            {hubText(report)}
          </text>
        </g>
      </svg>
      <div className="guide" style={{ top: guideTop, left: center.x, height: GUIDE_HEIGHT }}>
        {text}
      </div>
    </>
  );
}
