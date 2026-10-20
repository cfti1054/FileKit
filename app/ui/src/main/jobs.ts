// job:event → 화면용 작업 목록. 순수 함수.
import type { EngineId, ErrorInfo, FormatId, JobEvent } from "../shared/ipc";

export type JobStatus = "queued" | "running" | "done" | "failed" | "canceled";

export interface JobRow {
  job_id: number;
  batch_id: number;
  input: string;
  to: FormatId;
  status: JobStatus;
  /** 0..1, null = 진행 중이지만 비율을 모름 */
  progress: number | null;
  engine: EngineId | null;
  output: string | null;
  error: ErrorInfo | null;
}

export interface RejectedRow {
  batch_id: number;
  input: string;
  error: ErrorInfo;
}

export interface BatchResult {
  batch_id: number;
  succeeded: number;
  failed: number;
  canceled: number;
  rejected: number;
  exit_code: number;
}

export interface JobsState {
  jobs: JobRow[];
  rejected: RejectedRow[];
  batches: BatchResult[];
}

export const emptyJobs: JobsState = { jobs: [], rejected: [], batches: [] };

function patch(state: JobsState, job_id: number, change: Partial<JobRow>): JobsState {
  if (!state.jobs.some((j) => j.job_id === job_id)) return state;
  return { ...state, jobs: state.jobs.map((j) => (j.job_id === job_id ? { ...j, ...change } : j)) };
}

export function applyJobEvent(state: JobsState, ev: JobEvent): JobsState {
  switch (ev.type) {
    case "queued":
      if (state.jobs.some((j) => j.job_id === ev.job_id)) return state;
      return {
        ...state,
        jobs: [
          ...state.jobs,
          {
            job_id: ev.job_id,
            batch_id: ev.batch_id,
            input: ev.input,
            to: ev.to,
            status: "queued",
            progress: 0,
            engine: null,
            output: null,
            error: null,
          },
        ],
      };
    case "rejected":
      return { ...state, rejected: [...state.rejected, { batch_id: ev.batch_id, input: ev.input, error: ev.error }] };
    case "started":
      return patch(state, ev.job_id, { status: "running", engine: ev.engine });
    case "progress":
      return patch(state, ev.job_id, { status: "running", progress: ev.progress });
    case "done":
      return patch(state, ev.job_id, { status: "done", progress: 1, output: ev.output });
    case "failed":
      return patch(state, ev.job_id, { status: "failed", error: ev.error });
    case "canceled":
      return patch(state, ev.job_id, { status: "canceled" });
    case "batch_done":
      return {
        ...state,
        batches: [
          ...state.batches.filter((b) => b.batch_id !== ev.batch_id),
          {
            batch_id: ev.batch_id,
            succeeded: ev.succeeded,
            failed: ev.failed,
            canceled: ev.canceled,
            rejected: ev.rejected,
            exit_code: ev.exit_code,
          },
        ],
      };
    default: {
      const unreachable: never = ev;
      return unreachable;
    }
  }
}

export function isActive(row: JobRow): boolean {
  return row.status === "queued" || row.status === "running";
}

/** "42%" 또는 진행 중(비율 모름)이면 "처리 중" */
export function progressLabel(row: JobRow): string {
  switch (row.status) {
    case "queued":
      return "대기 중";
    case "running":
      return row.progress === null ? "처리 중" : `${Math.round(row.progress * 100)}%`;
    case "done":
      return "완료";
    case "failed":
      return "실패";
    case "canceled":
      return "취소됨";
  }
}
