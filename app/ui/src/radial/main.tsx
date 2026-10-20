import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { createIpc, isTauriRuntime } from "../shared/ipc";
import { MOCK_NAMES, makePayload, parseMockName } from "./fixtures";
import { RadialMenu } from "./RadialMenu";

const tauri = isTauriRuntime();
const search = window.location.search;
const name = parseMockName(search);

// Tauri가 없으면 mock IPC로 바로 메뉴가 열린다. 고르거나 취소하면 잠시 뒤 다시 열린다.
const ipc = createIpc(tauri ? undefined : { radialPayload: () => makePayload(name, search) });

const container = document.getElementById("root");
if (container) {
  if (!tauri) document.body.classList.add("dev-bg");
  createRoot(container).render(
    <StrictMode>
      <RadialMenu ipc={ipc} autoOpenMore={!tauri && name === "more"} />
      {!tauri && (
        <nav className="dev-bar" aria-label="mock 선택">
          {MOCK_NAMES.map((n) => (
            <a key={n} href={`?mock=${n}`} aria-current={n === name ? "true" : undefined}>
              {n}
            </a>
          ))}
        </nav>
      )}
    </StrictMode>,
  );
}
