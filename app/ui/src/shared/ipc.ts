// Tauri IPC 타입과 호출을 한 곳에 모은다. 필드 이름은 Rust serde 그대로 snake_case다.
// 근거: _workspace/01_architect_plan.md 「경계 인터페이스」 (1)·(3).
import { createMockIpc, type MockOptions } from "./mock";

export type FormatId = string;
export type Category = "video" | "audio" | "image" | "document" | "text";
export type EngineId = "ffmpeg" | "vips" | "office" | "pandoc";
export type PresetId = "web" | "original" | "small";
export type ErrorCode =
  | "unsupported_format"
  | "engine_missing"
  | "codec_missing"
  | "engine_failed"
  | "canceled"
  | "io"
  | "invalid_input"
  | "internal";
export interface ErrorInfo {
  code: ErrorCode;
  message: string;
  engine: EngineId | null;
}

export interface Unavailable {
  code: "engine_missing" | "codec_missing";
  engine: EngineId | null;
  message: string;
}
export interface FormatOption {
  id: FormatId;
  label: string;
  description: string;
  engines: EngineId[];
  available: boolean;
  unavailable: Unavailable | null;
}
export interface FileFormat {
  path: string;
  format: FormatId;
  category: Category;
}
export type SkipReason = "unsupported_format" | "different_category";
export interface SkippedFile {
  path: string;
  reason: SkipReason;
}
export type RadialSlot =
  | { kind: "format"; format_id: FormatId }
  | { kind: "more" }
  | { kind: "empty" };
/**
 * 칸(섹터) 번호 규약: 12시 = 0°, 시계 방향. 칸 i = [(270 + i*360/N) mod 360, +360/N).
 * 즉 9시 방향에서 시작해 시계 방향. N=4이면 0=왼쪽 위, 1=오른쪽 위, 2=오른쪽 아래, 3=왼쪽 아래.
 * "더 보기" 칸 = θ=135°가 들어 있는 칸 (N=4 → 2, N=8 → 5).
 */
export interface RadialLayout {
  slot_count: number;
  slots: RadialSlot[];
  more: FormatId[];
}
export interface FormatsReport {
  files: FileFormat[];
  skipped: SkippedFile[];
  options: FormatOption[];
  layout: RadialLayout;
}

export interface JobRequest {
  files: string[];
  to: FormatId;
  preset: PresetId;
  out_dir: string | null;
  overwrite: boolean;
}
export interface QueuedJob {
  job_id: number;
  input: string;
  to: FormatId;
}
export interface RejectedFile {
  input: string;
  error: ErrorInfo;
}
export interface BatchTicket {
  batch_id: number;
  jobs: QueuedJob[];
  rejected: RejectedFile[];
}
export type JobEvent =
  | { type: "queued"; batch_id: number; job_id: number; input: string; to: FormatId }
  | { type: "rejected"; batch_id: number; input: string; error: ErrorInfo }
  | { type: "started"; batch_id: number; job_id: number; engine: EngineId }
  | { type: "progress"; batch_id: number; job_id: number; progress: number | null }
  | { type: "done"; batch_id: number; job_id: number; input: string; output: string }
  | { type: "failed"; batch_id: number; job_id: number; input: string; error: ErrorInfo }
  | { type: "canceled"; batch_id: number; job_id: number; input: string }
  | {
      type: "batch_done";
      batch_id: number;
      succeeded: number;
      failed: number;
      canceled: number;
      rejected: number;
      exit_code: number;
    };

export type EngineSource = "bundled" | "config" | "path" | "system";
export interface EngineInfo {
  id: EngineId;
  name: string;
  found: boolean;
  path: string | null;
  source: EngineSource | null;
  version: string | null;
  required: boolean;
  notes: string[];
}
export interface PresetInfo {
  id: PresetId;
  label: string;
  description: string;
}

export interface Point {
  x: number;
  y: number;
}
export type GuidePosition = "below" | "above";
export type OpenedBy = "wheel" | "context_menu" | "hotkey";
export interface ThemeInfo {
  accent: string | null;
  animations_enabled: boolean;
  high_contrast: boolean;
}
export interface RadialOpenPayload {
  session_id: number;
  report: FormatsReport;
  center: Point;
  guide_position: GuidePosition;
  theme: ThemeInfo;
  opened_by: OpenedBy;
}
export interface RadialClosePayload {
  session_id: number;
  reason: "focus_lost" | "superseded" | "shutdown";
}
export interface MButtonUpPayload {
  session_id: number;
  x: number;
  y: number;
  held_ms: number;
}
export interface MainNavigatePayload {
  view: "drop" | "history" | "settings";
}

export const EVENTS = {
  radialOpen: "radial:open",
  radialClose: "radial:close",
  radialMButtonUp: "radial:mbutton_up",
  job: "job:event",
  theme: "theme:changed",
  mainNavigate: "main:navigate",
} as const;

export interface EventMap {
  "radial:open": RadialOpenPayload;
  "radial:close": RadialClosePayload;
  "radial:mbutton_up": MButtonUpPayload;
  "job:event": JobEvent;
  "theme:changed": ThemeInfo;
  "main:navigate": MainNavigatePayload;
}

export interface Ipc {
  getFormats(files: string[], primary: string | null): Promise<FormatsReport>;
  submitConvert(request: JobRequest): Promise<BatchTicket>;
  cancelJob(job_id: number): Promise<boolean>;
  getEngines(): Promise<EngineInfo[]>;
  getPresets(): Promise<PresetInfo[]>;
  getLaunchFiles(): Promise<string[]>;
  radialReady(): Promise<RadialOpenPayload | null>;
  radialSelect(session_id: number, format_id: FormatId): Promise<void>;
  radialCancel(session_id: number): Promise<void>;
  on<E extends keyof EventMap>(event: E, cb: (payload: EventMap[E]) => void): Promise<() => void>;
}

/** Tauri 창 안인지. 브라우저(vite dev)에서는 false. */
export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Tauri가 reject한 값(ErrorInfo)이나 일반 Error를 ErrorInfo로 맞춘다. */
export function toErrorInfo(e: unknown): ErrorInfo {
  if (typeof e === "object" && e !== null && "code" in e && "message" in e) {
    const o = e as { code: unknown; message: unknown; engine?: unknown };
    if (typeof o.code === "string" && typeof o.message === "string") {
      return {
        code: o.code as ErrorCode,
        message: o.message,
        engine: typeof o.engine === "string" ? (o.engine as EngineId) : null,
      };
    }
  }
  const message = e instanceof Error ? e.message : typeof e === "string" ? e : "알 수 없는 오류";
  return { code: "internal", message, engine: null };
}

async function createTauriIpc(): Promise<Ipc> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  // invoke 인자 객체는 snake_case 그대로 (Rust 쪽 rename_all = "snake_case").
  return {
    getFormats: (files, primary) => invoke<FormatsReport>("get_formats", { files, primary }),
    submitConvert: (request) => invoke<BatchTicket>("submit_convert", { request }),
    cancelJob: (job_id) => invoke<boolean>("cancel_job", { job_id }),
    getEngines: () => invoke<EngineInfo[]>("get_engines"),
    getPresets: () => invoke<PresetInfo[]>("get_presets"),
    getLaunchFiles: () => invoke<string[]>("get_launch_files"),
    radialReady: () => invoke<RadialOpenPayload | null>("radial_ready"),
    radialSelect: (session_id, format_id) => invoke<void>("radial_select", { session_id, format_id }),
    radialCancel: (session_id) => invoke<void>("radial_cancel", { session_id }),
    on: async (event, cb) => listen(event, (e) => cb(e.payload as never)),
  };
}

/**
 * IPC 구현을 고른다. Tauri 창이면 실제 invoke/listen, 아니면 mock.
 * Tauri 모듈은 동적으로 불러 브라우저 번들에서 실패하지 않게 한다.
 */
export function createIpc(mock?: MockOptions): Ipc {
  if (!isTauriRuntime()) return createMockIpc(mock);
  let real: Promise<Ipc> | null = null;
  const get = () => (real ??= createTauriIpc());
  return {
    getFormats: (f, p) => get().then((i) => i.getFormats(f, p)),
    submitConvert: (r) => get().then((i) => i.submitConvert(r)),
    cancelJob: (id) => get().then((i) => i.cancelJob(id)),
    getEngines: () => get().then((i) => i.getEngines()),
    getPresets: () => get().then((i) => i.getPresets()),
    getLaunchFiles: () => get().then((i) => i.getLaunchFiles()),
    radialReady: () => get().then((i) => i.radialReady()),
    radialSelect: (s, f) => get().then((i) => i.radialSelect(s, f)),
    radialCancel: (s) => get().then((i) => i.radialCancel(s)),
    on: (event, cb) => get().then((i) => i.on(event, cb)),
  };
}

/** 메인 창: 파일 끌어다 놓기. Tauri에서는 onDragDropEvent, 브라우저에서는 호출하는 쪽이 HTML5 drop을 쓴다. */
export async function onFilesDropped(
  cb: (paths: string[]) => void,
  hover?: (over: boolean) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => undefined;
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  return getCurrentWebview().onDragDropEvent((e) => {
    const p = e.payload;
    if (p.type === "enter" || p.type === "over") hover?.(true);
    else if (p.type === "leave") hover?.(false);
    else if (p.type === "drop") {
      hover?.(false);
      cb(p.paths);
    }
  });
}
