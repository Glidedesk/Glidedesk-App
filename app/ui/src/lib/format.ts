import type { HealthState, LinkState } from "./types";

export function healthLabel(s: HealthState): string {
  return {
    "never-connected": "Not connected yet",
    connecting: "Connecting",
    online: "Online",
    degraded: "Slow",
    locked: "Locked",
    offline: "Offline",
  }[s];
}

export function healthTone(s: HealthState): "ok" | "warn" | "muted" | "busy" {
  if (s === "online") return "ok";
  if (s === "degraded" || s === "locked") return "warn";
  if (s === "connecting") return "busy";
  return "muted";
}

export function linkLabel(s: LinkState): string {
  return {
    stopped: "Stopped",
    searching: "Searching for the server…",
    connecting: "Connecting…",
    connected: "Connected",
    rejected: "Refused by the server",
    offline: "Server offline",
  }[s];
}

export function ago(unix: number | null): string {
  if (!unix) return "never";
  const d = Math.max(0, Date.now() / 1000 - unix);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  return `${Math.round(d / 86400)} days ago`;
}

export function bytes(n: number): string {
  if (n === 0) return "unlimited";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v % 1 === 0 ? v : v.toFixed(1)} ${u[i]}`;
}
