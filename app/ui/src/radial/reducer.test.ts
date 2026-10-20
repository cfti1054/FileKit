import { describe, expect, it } from "vitest";
import type { RadialOpenPayload, ThemeInfo } from "../shared/ipc";
import { mockLayout, mockOption } from "../shared/mock";
import {
  guideText,
  hubText,
  initialState,
  openingAnnouncement,
  reducer,
  type RadialAction,
  type RadialState,
} from "./reducer";

const theme: ThemeInfo = { accent: null, animations_enabled: true, high_contrast: false };

function payload(over: Partial<RadialOpenPayload> = {}, files = ["C:\\영상\\발표녹화.mov"]): RadialOpenPayload {
  const options = [
    mockOption("mp4", "영상 (H.264)"),
    mockOption("mp3", "음성 추출"),
    mockOption("gif", "움직이는 이미지"),
    mockOption("webm", "영상 (VP9)"),
  ];
  return {
    session_id: 7,
    report: {
      files: files.map((path) => ({ path, format: "mov", category: "video" as const })),
      skipped: [],
      options,
      layout: mockLayout(options.map((o) => o.id), 4),
    },
    center: { x: 240, y: 240 },
    guide_position: "below",
    theme,
    opened_by: "context_menu",
    ...over,
  };
}

function run(actions: RadialAction[], start: RadialState = initialState): RadialState {
  return actions.reduce(reducer, start);
}

// mockLayout(4개, 4칸): 0=mp4(왼쪽 위), 1=mp3(오른쪽 위), 2=더 보기(오른쪽 아래), 3=gif(왼쪽 아래), more=[webm]
describe("reducer: 열기와 낭독", () => {
  it("열면 phase=open, 낭독 문구는 '변환 메뉴, 파일, 첫 항목 1/4'", () => {
    const s = run([{ type: "open", payload: payload() }]);
    expect(s.phase).toBe("open");
    expect(s.announcement).toBe("변환 메뉴, 발표녹화.mov, MP4 1/4");
    expect(openingAnnouncement(payload())).toBe("변환 메뉴, 발표녹화.mov, MP4 1/4");
  });

  it("허브 문구: 1개면 대문자 확장자, 여러 개면 N개 파일", () => {
    expect(hubText(payload().report)).toBe("MOV");
    expect(hubText(payload({}, ["a.mov", "b.mov", "c.mov"]).report)).toBe("3개 파일");
  });
});

describe("reducer: 닫을 때 상태 초기화", () => {
  it("Rust가 닫으면 강조 칸, 두 번째 고리, 선택 정보를 모두 지운다 (테마는 유지)", () => {
    const dirty = run([
      { type: "open", payload: payload() },
      { type: "pointer_move", x: 340, y: 140 },
      { type: "activate", target: { ring: "main", index: 2 } },
    ]);
    expect(dirty.active).not.toBeNull();
    expect(dirty.moreOpen).toBe(true);
    expect(dirty.leftDeadZone).toBe(true);
    const closed = reducer(dirty, { type: "remote_close", payload: { session_id: 7, reason: "focus_lost" } });
    expect(closed).toEqual({ ...initialState, theme });
  });

  it("고른 뒤 finish하면 초기화", () => {
    const s = run([
      { type: "open", payload: payload() },
      { type: "activate", target: { ring: "main", index: 0 } },
    ]);
    expect(s.phase).toBe("choosing");
    expect(s.chosen).toEqual({ format_id: "mp4", target: { ring: "main", index: 0 } });
    expect(reducer(s, { type: "finish", session_id: 7 })).toEqual({ ...initialState, theme });
  });

  it("오래된 세션의 close/finish는 무시한다", () => {
    const open = run([{ type: "open", payload: payload({ session_id: 8 }) }]);
    expect(reducer(open, { type: "remote_close", payload: { session_id: 7, reason: "superseded" } })).toBe(open);
    expect(reducer(open, { type: "finish", session_id: 7 })).toBe(open);
  });

  it("다시 열면 이전 강조·고리가 남지 않는다", () => {
    const s = run([
      { type: "open", payload: payload() },
      { type: "activate", target: { ring: "main", index: 2 } },
      { type: "open", payload: payload({ session_id: 9 }) },
    ]);
    expect(s.moreOpen).toBe(false);
    expect(s.active).toBeNull();
    expect(s.session?.session_id).toBe(9);
  });
});

describe("reducer: 고르기와 취소", () => {
  const open = run([{ type: "open", payload: payload() }]);

  it("바깥 클릭은 취소", () => {
    expect(reducer(open, { type: "press", x: 240 + 150, y: 240 }).phase).toBe("canceling");
  });

  it("칸 클릭은 고르기, 데드존 클릭은 무시", () => {
    expect(reducer(open, { type: "press", x: 240 - 70, y: 240 - 70 }).chosen?.format_id).toBe("mp4");
    expect(reducer(open, { type: "press", x: 240, y: 240 })).toBe(open);
  });

  it("더 보기 칸을 고르면 고리가 열리고 다시 고르면 닫힌다", () => {
    const a = reducer(open, { type: "activate", target: { ring: "main", index: 2 } });
    expect(a.moreOpen).toBe(true);
    expect(a.phase).toBe("open");
    const b = reducer(a, { type: "activate", target: { ring: "main", index: 2 } });
    expect(b.moreOpen).toBe(false);
  });

  it("두 번째 고리의 항목 고르기", () => {
    const a = reducer(open, { type: "activate", target: { ring: "main", index: 2 } });
    const b = reducer(a, { type: "activate", target: { ring: "more", index: 0 } });
    expect(b.chosen?.format_id).toBe("webm");
  });

  it("choosing 중에는 다른 입력을 받지 않는다", () => {
    const c = reducer(open, { type: "activate", target: { ring: "main", index: 0 } });
    expect(reducer(c, { type: "cancel" })).toBe(c);
    expect(reducer(c, { type: "activate", target: { ring: "main", index: 1 } })).toBe(c);
  });

  it("Esc/cancel은 canceling", () => {
    expect(reducer(open, { type: "cancel" }).phase).toBe("canceling");
  });
});

describe("reducer: 사용 불가 칸", () => {
  const p = payload();
  p.report.options[1] = mockOption("mp3", "음성 추출", "LibreOffice가 설치되지 않았습니다");
  const open = run([{ type: "open", payload: p }]);

  it("고를 수 없고, 안내 줄에 이유가 나온다", () => {
    const s = reducer(open, { type: "activate", target: { ring: "main", index: 1 } });
    expect(s.phase).toBe("open");
    expect(s.chosen).toBeNull();
    expect(guideText(s)).toBe("LibreOffice가 설치되지 않았습니다");
    expect(s.announcement).toContain("사용할 수 없음");
  });

  it("커서가 올라가면 안내 줄이 바뀌고, 벗어나면 파일 이름으로 돌아온다", () => {
    const hover = reducer(open, { type: "pointer_move", x: 240 + 70, y: 240 - 70 });
    expect(hover.active).toEqual({ ring: "main", index: 1 });
    expect(guideText(hover)).toBe("LibreOffice가 설치되지 않았습니다");
    const away = reducer(hover, { type: "pointer_move", x: 240, y: 240 });
    expect(guideText(away)).toBe("발표녹화.mov");
  });
});

describe("reducer: 안내 줄", () => {
  it("정상 칸에 커서가 있으면 그 형식의 설명", () => {
    const s = run([
      { type: "open", payload: payload() },
      { type: "pointer_move", x: 240 - 70, y: 240 - 70 },
    ]);
    expect(guideText(s)).toBe("영상 (H.264)");
  });
  it("여러 파일이면 '첫 파일 외 N개'", () => {
    const s = run([{ type: "open", payload: payload({}, ["C:\\a.mov", "C:\\b.mov", "C:\\c.mov"]) }]);
    expect(guideText(s)).toBe("a.mov 외 2개");
  });
});

describe("reducer: 제스처 (radial:mbutton_up)", () => {
  const open = run([{ type: "open", payload: payload() }]);
  const up = (x: number, y: number, held_ms: number, session_id = 7): RadialAction => ({
    type: "gesture_release",
    payload: { session_id, x, y, held_ms },
  });

  it("데드존 밖에서 떼면 그 칸을 고른다", () => {
    expect(reducer(open, up(240 + 80, 240 - 80, 600)).chosen?.format_id).toBe("mp3");
  });
  it("데드존을 벗어난 적 없고 250ms 미만이면 열어 둔다", () => {
    expect(reducer(open, up(240, 240, 120))).toBe(open);
  });
  it("250ms 이상 데드존에서 떼면 취소", () => {
    expect(reducer(open, up(240, 240, 400)).phase).toBe("canceling");
  });
  it("포인터가 데드존 밖으로 나갔다 돌아와 떼면 취소", () => {
    const moved = run([{ type: "pointer_move", x: 340, y: 240 }, { type: "pointer_move", x: 240, y: 240 }], open);
    expect(moved.leftDeadZone).toBe(true);
    expect(reducer(moved, up(240, 240, 100)).phase).toBe("canceling");
  });
  it("다른 세션의 이벤트는 무시", () => {
    expect(reducer(open, up(320, 160, 600, 99))).toBe(open);
  });
  it("제스처로 더 보기 칸에서 떼면 고리가 열린다", () => {
    const s = reducer(open, up(240 + 80, 240 + 80, 600));
    expect(s.phase).toBe("open");
    expect(s.moreOpen).toBe(true);
  });
});
