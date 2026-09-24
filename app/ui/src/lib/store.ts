// Live agent status (pushed by the Rust side) with a retrying initial fetch.
import { useSyncExternalStore } from "react";
import { api, onStatus } from "./api";
import type { AgentStatus } from "./types";

let current: AgentStatus | null = null;
const listeners = new Set<() => void>();

function set(s: AgentStatus) {
  current = s;
  for (const l of listeners) l();
}

let started = false;
let unlisten: (() => void) | null = null;

function start() {
  if (started) return;
  started = true;
  void onStatus(set).then((u) => {
    unlisten = u;
  });
  // The agent may still be starting: retry until the first status arrives.
  const fetchOnce = (delay: number) => {
    if (current) return;
    api.status().then(
      (s) => {
        if (s.version) set(s);
        else setTimeout(() => fetchOnce(Math.min(delay * 2, 2000)), delay);
      },
      () => setTimeout(() => fetchOnce(Math.min(delay * 2, 2000)), delay),
    );
  };
  fetchOnce(200);
}

// Vite hot reload: drop the old listener instead of stacking duplicates.
if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    unlisten?.();
    started = false;
  });
}

export function useStatus(): AgentStatus | null {
  start();
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => current,
  );
}
