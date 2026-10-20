import { useEffect, useRef, useState, type DragEvent } from "react";
import { isTauriRuntime, onFilesDropped } from "../shared/ipc";

interface Props {
  onFiles: (paths: string[]) => void;
  disabled?: boolean;
}

/**
 * 끌어다 놓기 영역.
 * Tauri: getCurrentWebview().onDragDropEvent로 실제 경로를 받는다.
 * 브라우저: HTML5 drop은 경로를 주지 않으므로 파일 이름만 mock에 넘긴다.
 */
export function DropZone({ onFiles, disabled = false }: Props) {
  const [over, setOver] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const tauri = isTauriRuntime();
  const latest = useRef(onFiles);
  useEffect(() => {
    latest.current = onFiles;
  }, [onFiles]);

  useEffect(() => {
    let off: (() => void) | undefined;
    let disposed = false;
    void onFilesDropped((paths) => latest.current(paths), setOver).then((u) => {
      if (disposed) u();
      else off = u;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    setOver(false);
    if (tauri) return; // Tauri는 onDragDropEvent로 받는다
    const names = Array.from(e.dataTransfer.files, (f) => f.name);
    if (names.length > 0) onFiles(names);
  };

  return (
    <div
      className={`dropzone${over ? " is-over" : ""}${disabled ? " is-disabled" : ""}`}
      onDragOver={(e) => {
        e.preventDefault();
        if (!tauri) setOver(true);
      }}
      onDragLeave={() => {
        if (!tauri) setOver(false);
      }}
      onDrop={onDrop}
    >
      <p className="dropzone-title">여기에 파일을 끌어다 놓으세요</p>
      <p className="dropzone-sub">영상, 음성, 이미지, 문서를 다른 형식으로 바꿉니다</p>
      {!tauri && (
        <>
          <button type="button" className="btn" onClick={() => input.current?.click()}>
            파일 고르기 (mock)
          </button>
          <input
            ref={input}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              const names = Array.from(e.target.files ?? [], (f) => f.name);
              e.target.value = "";
              if (names.length > 0) onFiles(names);
            }}
          />
        </>
      )}
    </div>
  );
}
