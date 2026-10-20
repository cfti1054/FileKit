import { describe, expect, it } from "vitest";
import type { ErrorInfo, JobEvent } from "../shared/ipc";
import { applyJobEvent, emptyJobs, isActive, progressLabel, type JobsState } from "./jobs";

const err: ErrorInfo = { code: "engine_failed", message: "실패", engine: "ffmpeg" };

function feed(events: JobEvent[]): JobsState {
  return events.reduce(applyJobEvent, emptyJobs);
}

const queued: JobEvent = { type: "queued", batch_id: 1, job_id: 1, input: "C:\\영상\\발표 녹화.mov", to: "mp4" };

describe("JobEvent 판별 유니온 처리", () => {
  it("queued → started → progress → done", () => {
    const s = feed([
      queued,
      { type: "started", batch_id: 1, job_id: 1, engine: "ffmpeg" },
      { type: "progress", batch_id: 1, job_id: 1, progress: 0.42 },
    ]);
    expect(s.jobs).toHaveLength(1);
    expect(s.jobs[0]).toMatchObject({ status: "running", engine: "ffmpeg", progress: 0.42 });
    expect(progressLabel(s.jobs[0]!)).toBe("42%");
    const d = applyJobEvent(s, { type: "done", batch_id: 1, job_id: 1, input: queued.input, output: "C:\\영상\\발표 녹화.mp4" });
    expect(d.jobs[0]).toMatchObject({ status: "done", progress: 1, output: "C:\\영상\\발표 녹화.mp4" });
    expect(isActive(d.jobs[0]!)).toBe(false);
  });

  it("진행률 null = 비율 모름", () => {
    const s = feed([queued, { type: "progress", batch_id: 1, job_id: 1, progress: null }]);
    expect(progressLabel(s.jobs[0]!)).toBe("처리 중");
  });

  it("failed / canceled", () => {
    const f = feed([queued, { type: "failed", batch_id: 1, job_id: 1, input: queued.input, error: err }]);
    expect(f.jobs[0]).toMatchObject({ status: "failed", error: err });
    const c = feed([queued, { type: "canceled", batch_id: 1, job_id: 1, input: queued.input }]);
    expect(c.jobs[0]?.status).toBe("canceled");
  });

  it("rejected는 작업 목록과 따로 쌓인다", () => {
    const s = feed([{ type: "rejected", batch_id: 1, input: "C:\\a.xyz", error: { ...err, code: "unsupported_format", engine: null } }]);
    expect(s.jobs).toHaveLength(0);
    expect(s.rejected[0]?.error.code).toBe("unsupported_format");
  });

  it("batch_done은 같은 batch_id를 덮어쓴다", () => {
    const e: JobEvent = { type: "batch_done", batch_id: 1, succeeded: 1, failed: 0, canceled: 0, rejected: 1, exit_code: 1 };
    const s = feed([e, e]);
    expect(s.batches).toEqual([{ batch_id: 1, succeeded: 1, failed: 0, canceled: 0, rejected: 1, exit_code: 1 }]);
  });

  it("모르는 job_id의 이벤트는 무시, 같은 queued 중복은 무시", () => {
    const s = feed([{ type: "progress", batch_id: 1, job_id: 99, progress: 0.5 }]);
    expect(s).toBe(emptyJobs);
    expect(feed([queued, queued]).jobs).toHaveLength(1);
  });
});
