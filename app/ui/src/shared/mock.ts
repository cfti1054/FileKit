// Tauri가 없는 브라우저에서 화면을 띄워 보기 위한 가짜 IPC.
// ipc.ts의 타입만 가져온다(값 import 금지: 순환 방지).
import type {
  BatchTicket,
  Category,
  EngineInfo,
  EventMap,
  FileFormat,
  FormatOption,
  FormatsReport,
  Ipc,
  JobEvent,
  JobRequest,
  PresetInfo,
  RadialLayout,
  RadialOpenPayload,
  RadialSlot,
} from "./ipc";
import { extOf } from "./path";

export interface MockOptions {
  /** 원형 메뉴: 열 때마다 새 payload를 만든다. 없으면 radialReady는 null. */
  radialPayload?: () => RadialOpenPayload;
  /** 고르기·취소 후 다시 여는 지연(ms). 0이면 다시 열지 않는다. */
  reopenAfterMs?: number;
}

const CATEGORY_BY_EXT: Record<string, Category> = {
  mp4: "video", mov: "video", avi: "video", mkv: "video", webm: "video",
  mp3: "audio", wav: "audio", m4a: "audio", flac: "audio", aac: "audio", ogg: "audio",
  jpg: "image", jpeg: "image", png: "image", webp: "image", heic: "image", bmp: "image", tiff: "image", gif: "image",
  docx: "document", pptx: "document", xlsx: "document", odt: "document", rtf: "document",
  md: "text", html: "text",
};

const OUTPUTS: Record<Category, [string, string][]> = {
  video: [["mp4", "영상 (H.264)"], ["mp3", "음성 추출"], ["gif", "움직이는 이미지"], ["webm", "영상 (VP9)"]],
  audio: [["mp3", "음성"], ["wav", "무손실 음성"], ["flac", "무손실 압축 음성"]],
  image: [["jpg", "사진"], ["png", "무손실 이미지"], ["webp", "웹용 이미지"]],
  document: [["pdf", "PDF 문서"], ["txt", "텍스트만"]],
  text: [["pdf", "PDF 문서"], ["docx", "Word 문서"], ["html", "웹 페이지"]],
};

/** 칸 번호 규약에 따른 기본 방향 배치(mock 전용). 더 보기 칸 = 135°가 들어 있는 칸. */
export function mockLayout(ids: string[], slotCount: number): RadialLayout {
  const moreIndex = Math.floor((((135 - 270 + 360) % 360) / 360) * slotCount);
  const slots: RadialSlot[] = Array.from({ length: slotCount }, () => ({ kind: "empty" }));
  const order = Array.from({ length: slotCount }, (_, i) => i).filter((i) => i !== moreIndex);
  let more: string[] = [];
  if (ids.length > order.length) {
    more = ids.slice(order.length);
    ids = ids.slice(0, order.length);
    slots[moreIndex] = { kind: "more" };
  }
  ids.forEach((id, k) => {
    const slot = order[k];
    if (slot !== undefined) slots[slot] = { kind: "format", format_id: id };
  });
  return { slot_count: slotCount, slots, more };
}

export function mockOption(
  id: string,
  description: string,
  unavailableMessage?: string,
): FormatOption {
  return {
    id,
    label: id.toUpperCase(),
    description,
    engines: ["ffmpeg"],
    available: unavailableMessage === undefined,
    unavailable:
      unavailableMessage === undefined
        ? null
        : { code: "engine_missing", engine: "office", message: unavailableMessage },
  };
}

/** 확장자만 보고 mock FormatsReport를 만든다. 첫 파일과 같은 분류만 남기고 교집합을 계산한다. */
export function mockReport(files: string[], primary: string | null, slotCount = 4): FormatsReport {
  const first = primary ?? files[0];
  const cat = first ? CATEGORY_BY_EXT[extOf(first)] : undefined;
  const kept: FileFormat[] = [];
  const skipped: FormatsReport["skipped"] = [];
  for (const path of files) {
    const c = CATEGORY_BY_EXT[extOf(path)];
    if (c === undefined) skipped.push({ path, reason: "unsupported_format" });
    else if (c !== cat) skipped.push({ path, reason: "different_category" });
    else kept.push({ path, format: extOf(path), category: c });
  }
  const table = cat ? OUTPUTS[cat] : [];
  const inputs = new Set(kept.map((f) => f.format));
  const options = table
    .filter(([id]) => !inputs.has(id))
    .map(([id, desc]) =>
      mockOption(id, desc, id === "pdf" ? "LibreOffice가 설치되지 않았습니다 (mock)" : undefined),
    );
  return {
    files: kept,
    skipped,
    options,
    layout: mockLayout(options.map((o) => o.id), slotCount),
  };
}

type Listener = (payload: never) => void;

export interface MockIpc extends Ipc {
  emit<E extends keyof EventMap>(event: E, payload: EventMap[E]): void;
}

export function createMockIpc(opts: MockOptions = {}): MockIpc {
  const listeners = new Map<string, Set<Listener>>();
  let nextBatch = 1;
  let nextJob = 1;

  const emit = <E extends keyof EventMap>(event: E, payload: EventMap[E]) => {
    listeners.get(event)?.forEach((cb) => cb(payload as never));
  };

  const reopen = () => {
    const ms = opts.reopenAfterMs ?? 700;
    if (ms > 0 && opts.radialPayload) {
      const make = opts.radialPayload;
      setTimeout(() => emit("radial:open", make()), ms);
    }
  };

  const runBatch = (request: JobRequest): BatchTicket => {
    const batch_id = nextBatch++;
    const jobs = request.files.map((input) => ({ job_id: nextJob++, input, to: request.to }));
    const send = (e: JobEvent) => emit("job:event", e);
    jobs.forEach((j, n) => {
      send({ type: "queued", batch_id, job_id: j.job_id, input: j.input, to: j.to });
      setTimeout(() => send({ type: "started", batch_id, job_id: j.job_id, engine: "ffmpeg" }), 150);
      for (let step = 1; step <= 4; step++) {
        setTimeout(
          () => send({ type: "progress", batch_id, job_id: j.job_id, progress: step / 5 }),
          150 + step * 300 + n * 100,
        );
      }
      setTimeout(
        () =>
          send({
            type: "done",
            batch_id,
            job_id: j.job_id,
            input: j.input,
            output: j.input.replace(/\.[^.\\/]+$/, "") + "." + request.to,
          }),
        1700 + n * 100,
      );
    });
    setTimeout(
      () =>
        send({
          type: "batch_done",
          batch_id,
          succeeded: jobs.length,
          failed: 0,
          canceled: 0,
          rejected: 0,
          exit_code: 0,
        }),
      1800 + jobs.length * 100,
    );
    return { batch_id, jobs, rejected: [] };
  };

  const engines: EngineInfo[] = [
    { id: "ffmpeg", name: "FFmpeg", found: true, path: null, source: "bundled", version: "mock", required: true, notes: [] },
    { id: "vips", name: "libvips", found: true, path: null, source: "bundled", version: "mock", required: true, notes: [] },
    { id: "office", name: "LibreOffice", found: false, path: null, source: null, version: null, required: false, notes: [] },
    { id: "pandoc", name: "Pandoc", found: false, path: null, source: null, version: null, required: false, notes: [] },
  ];
  const presets: PresetInfo[] = [
    { id: "web", label: "웹용", description: "화질과 크기의 균형" },
    { id: "original", label: "원본 화질", description: "화질 우선" },
    { id: "small", label: "작은 크기", description: "크기 우선" },
  ];

  return {
    emit,
    getFormats: async (files, primary) => mockReport(files, primary),
    submitConvert: async (request) => runBatch(request),
    cancelJob: async () => false,
    getEngines: async () => engines,
    getPresets: async () => presets,
    getLaunchFiles: async () => [],
    radialReady: async () => opts.radialPayload?.() ?? null,
    radialSelect: async (session_id, format_id) => {
      console.info("[mock] radial_select", { session_id, format_id });
      reopen();
    },
    radialCancel: async (session_id) => {
      console.info("[mock] radial_cancel", { session_id });
      reopen();
    },
    on: async (event, cb) => {
      const set = listeners.get(event) ?? new Set<Listener>();
      listeners.set(event, set);
      const l = cb as Listener;
      set.add(l);
      return () => {
        set.delete(l);
      };
    },
  };
}
