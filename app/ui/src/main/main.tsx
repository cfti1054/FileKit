import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { createIpc } from "../shared/ipc";
import { App } from "./App";

const ipc = createIpc();
const container = document.getElementById("root");
if (container) {
  createRoot(container).render(
    <StrictMode>
      <App ipc={ipc} />
    </StrictMode>,
  );
}
