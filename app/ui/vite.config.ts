import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { agentBridge } from "./dev/agent-bridge";

export default defineConfig({
  // agentBridge: dev server only, off unless GLIDEDESK_DEV_AGENTS is set (browser QA).
  plugins: [react(), tailwindcss(), agentBridge()],
  clearScreen: false,
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    // One small bundle: no code splitting needed for a settings window.
    chunkSizeWarningLimit: 600,
  },
  test: {
    environment: "node",
  },
});
