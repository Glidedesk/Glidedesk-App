import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
import { App } from "./App";

async function start() {
  // `vite dev` in a normal browser: talk to a real agent through the dev bridge.
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    const { installDevShim } = await import("./lib/devshim");
    await installDevShim();
  }
  const root = document.getElementById("root");
  if (root) {
    createRoot(root).render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
  }
}

void start();
