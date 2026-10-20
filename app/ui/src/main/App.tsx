import { useCallback, useEffect, useReducer, useState } from "react";
import {
  toErrorInfo,
  type EngineInfo,
  type ErrorInfo,
  type FormatOption,
  type FormatsReport,
  type Ipc,
  type MainNavigatePayload,
  type PresetId,
  type PresetInfo,
} from "../shared/ipc";
import { baseName } from "../shared/path";
import { DropZone } from "./DropZone";
import { applyJobEvent, emptyJobs, isActive, progressLabel } from "./jobs";
import "./main.css";

type View = MainNavigatePayload["view"];

const TABS: Array<{ id: View; label: string }> = [
  { id: "drop", label: "변환" },
  { id: "history", label: "기록" },
  { id: "settings", label: "설정" },
];

export function App({ ipc }: { ipc: Ipc }) {
  const [view, setView] = useState<View>("drop");
  const [report, setReport] = useState<FormatsReport | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const [preset, setPreset] = useState<PresetId>("web");
  const [jobs, dispatchJob] = useReducer(applyJobEvent, emptyJobs);

  const handleFiles = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) return;
      setBusy(true);
      setError(null);
      try {
        setReport(await ipc.getFormats(paths, paths[0] ?? null));
      } catch (e) {
        setReport(null);
        setError(toErrorInfo(e));
      } finally {
        setBusy(false);
      }
    },
    [ipc],
  );

  // 이벤트 구독 + 시작할 때 받은 파일 읽기
  useEffect(() => {
    let disposed = false;
    const offs: Array<() => void> = [];
    void Promise.all([
      ipc.on("job:event", dispatchJob),
      ipc.on("main:navigate", (p) => setView(p.view)),
    ]).then((us) => {
      if (disposed) us.forEach((u) => u());
      else offs.push(...us);
    });
    ipc.getPresets().then((p) => !disposed && setPresets(p)).catch(() => undefined);
    ipc
      .getLaunchFiles()
      .then((files) => {
        if (!disposed && files.length > 0) void handleFiles(files);
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      offs.forEach((u) => u());
    };
  }, [ipc, handleFiles]);

  const convert = async (option: FormatOption) => {
    if (!report || !option.available) return;
    setError(null);
    try {
      const ticket = await ipc.submitConvert({
        files: report.files.map((f) => f.path),
        to: option.id,
        preset,
        out_dir: null,
        overwrite: false,
      });
      ticket.rejected.forEach((r) =>
        dispatchJob({ type: "rejected", batch_id: ticket.batch_id, input: r.input, error: r.error }),
      );
      setReport(null);
    } catch (e) {
      setError(toErrorInfo(e));
    }
  };

  return (
    <div className="app">
      <header className="app-header">
        <h1>변환툴</h1>
        <nav className="tabs" role="tablist" aria-label="화면">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={view === t.id}
              className={`tab${view === t.id ? " is-selected" : ""}`}
              onClick={() => setView(t.id)}
            >
              {t.label}
            </button>
          ))}
        </nav>
      </header>

      <main className="app-body">
        {view === "drop" && (
          <>
            <DropZone onFiles={(p) => void handleFiles(p)} disabled={busy} />
            {error && (
              <p className="notice is-error" role="alert">
                {error.message}
              </p>
            )}
            {report && (
              <FormatPicker
                report={report}
                presets={presets}
                preset={preset}
                onPreset={setPreset}
                onPick={(o) => void convert(o)}
              />
            )}
            <JobList state={jobs} onCancel={(id) => void ipc.cancelJob(id)} />
          </>
        )}
        {view === "history" && (
          <section className="placeholder">
            <h2>변환 기록</h2>
            <p>최근 변환한 파일이 여기에 표시됩니다. (준비 중)</p>
          </section>
        )}
        {view === "settings" && <Settings ipc={ipc} />}
      </main>
    </div>
  );
}

function FormatPicker(props: {
  report: FormatsReport;
  presets: PresetInfo[];
  preset: PresetId;
  onPreset: (p: PresetId) => void;
  onPick: (o: FormatOption) => void;
}) {
  const { report } = props;
  const count = report.files.length;
  return (
    <section className="picker" aria-label="변환할 형식">
      <h2>
        {count === 1 ? baseName(report.files[0]?.path ?? "") : `${count}개 파일`}
        <span className="muted"> 을(를) 무엇으로 바꿀까요?</span>
      </h2>
      {report.skipped.length > 0 && (
        <p className="notice">
          {report.skipped.length}개 파일은 제외했습니다 (지원하지 않거나 종류가 다름):{" "}
          {report.skipped.map((s) => baseName(s.path)).join(", ")}
        </p>
      )}
      <div className="format-grid">
        {report.options.map((o) => (
          <button
            key={o.id}
            type="button"
            className="format-btn"
            disabled={!o.available}
            title={o.available ? o.description : (o.unavailable?.message ?? "")}
            onClick={() => props.onPick(o)}
          >
            <strong>{o.label}</strong>
            <span>{o.available ? o.description : (o.unavailable?.message ?? "사용할 수 없음")}</span>
          </button>
        ))}
      </div>
      {props.presets.length > 0 && (
        <label className="preset">
          품질
          <select value={props.preset} onChange={(e) => props.onPreset(e.target.value as PresetId)}>
            {props.presets.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label} - {p.description}
              </option>
            ))}
          </select>
        </label>
      )}
    </section>
  );
}

function JobList({ state, onCancel }: { state: typeof emptyJobs; onCancel: (id: number) => void }) {
  if (state.jobs.length === 0 && state.rejected.length === 0) return null;
  return (
    <section className="jobs" aria-label="변환 진행">
      <h2>진행 상황</h2>
      <ul>
        {state.jobs.map((j) => (
          <li key={j.job_id} className={`job is-${j.status}`}>
            <span className="job-name">
              {baseName(j.input)} <span className="muted">→ {j.to.toUpperCase()}</span>
            </span>
            <progress
              max={1}
              value={j.status === "running" && j.progress === null ? undefined : (j.progress ?? 0)}
              aria-label={`${baseName(j.input)} 진행률`}
            />
            <span className="job-state">{progressLabel(j)}</span>
            {isActive(j) && (
              <button type="button" className="btn" onClick={() => onCancel(j.job_id)}>
                취소
              </button>
            )}
            {j.error && <span className="job-error">{j.error.message}</span>}
            {j.output && <span className="job-output muted">{j.output}</span>}
          </li>
        ))}
        {state.rejected.map((r, i) => (
          <li key={`r${i}`} className="job is-failed">
            <span className="job-name">{baseName(r.input)}</span>
            <span className="job-error">{r.error.message}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Settings({ ipc }: { ipc: Ipc }) {
  const [engines, setEngines] = useState<EngineInfo[] | null>(null);
  useEffect(() => {
    let alive = true;
    ipc.getEngines().then((e) => alive && setEngines(e)).catch(() => alive && setEngines([]));
    return () => {
      alive = false;
    };
  }, [ipc]);
  return (
    <section className="placeholder">
      <h2>설정</h2>
      <p>원형 메뉴 방향 배치, 출력 폴더, 품질 기본값은 여기서 바꿉니다. (준비 중)</p>
      <h3>변환 엔진</h3>
      {engines === null ? (
        <p className="muted">확인 중...</p>
      ) : (
        <ul className="engines">
          {engines.map((e) => (
            <li key={e.id}>
              <strong>{e.name}</strong> {e.found ? (e.version ?? "찾음") : e.required ? "없음 (필수)" : "없음"}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
