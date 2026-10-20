// 브라우저에서 radial.html?mock=… 로 볼 수 있는 가짜 데이터.
import type { FormatsReport, RadialOpenPayload, ThemeInfo } from "../shared/ipc";
import { mockLayout, mockOption } from "../shared/mock";

export const MOCK_NAMES = ["four", "eight", "multi", "disabled", "more"] as const;
export type MockName = (typeof MOCK_NAMES)[number];

export function parseMockName(search: string): MockName {
  const v = new URLSearchParams(search).get("mock");
  return (MOCK_NAMES as readonly string[]).includes(v ?? "") ? (v as MockName) : "four";
}

const VIDEO_OPTIONS = [
  mockOption("mp4", "영상 (H.264)"),
  mockOption("mp3", "음성 추출"),
  mockOption("gif", "움직이는 이미지"),
  mockOption("webm", "영상 (VP9)"),
];

function videoReport(files: string[], formats: string[], slotCount: number, options = VIDEO_OPTIONS): FormatsReport {
  return {
    files: files.map((path, i) => ({ path, format: formats[i] ?? "mov", category: "video" as const })),
    skipped: [],
    options,
    layout: mockLayout(options.map((o) => o.id), slotCount),
  };
}

function reportFor(name: MockName): FormatsReport {
  switch (name) {
    case "four":
    case "more":
      return videoReport(["C:\\영상\\발표녹화.mov"], ["mov"], 4);
    case "multi":
      return videoReport(
        ["C:\\영상\\발표녹화.mov", "C:\\영상\\회의.mp4", "C:\\영상\\여행.mkv"],
        ["mov", "mp4", "mkv"],
        4,
        VIDEO_OPTIONS.filter((o) => o.id !== "mp4"),
      );
    case "eight": {
      const options = [
        mockOption("mp4", "영상 (H.264)"),
        mockOption("mov", "QuickTime"),
        mockOption("mkv", "Matroska"),
        mockOption("webm", "영상 (VP9)"),
        mockOption("gif", "움직이는 이미지"),
        mockOption("mp3", "음성 추출"),
        mockOption("wav", "무손실 음성"),
        mockOption("flac", "무손실 압축 음성"),
        mockOption("avi", "AVI"),
      ];
      return videoReport(["C:\\영상\\긴 이름의 발표 녹화 파일.ogv"], ["ogv"], 8, options);
    }
    case "disabled": {
      const options = [
        mockOption("pdf", "PDF 문서", "LibreOffice가 설치되지 않았습니다"),
        mockOption("docx", "Word 문서"),
        mockOption("html", "웹 페이지"),
      ];
      return {
        files: [{ path: "C:\\문서\\회의록.md", format: "md", category: "text" }],
        skipped: [],
        options,
        layout: mockLayout(options.map((o) => o.id), 4),
      };
    }
  }
}

export function mockTheme(search: string): ThemeInfo {
  const q = new URLSearchParams(search);
  const accent = q.get("accent");
  return {
    accent: accent && /^#?[0-9a-f]{6}$/i.test(accent) ? (accent.startsWith("#") ? accent : `#${accent}`) : null,
    animations_enabled: q.get("motion") !== "0",
    high_contrast: false,
  };
}

let sessionCounter = 0;

/** 열 때마다 새 session_id를 붙인 payload. ?guide=above, ?accent=%23FFB900, ?motion=0 지원 */
export function makePayload(name: MockName, search: string): RadialOpenPayload {
  const q = new URLSearchParams(search);
  return {
    session_id: ++sessionCounter,
    report: reportFor(name),
    center: { x: 240, y: 240 },
    guide_position: q.get("guide") === "above" ? "above" : "below",
    theme: mockTheme(search),
    opened_by: "context_menu",
  };
}
