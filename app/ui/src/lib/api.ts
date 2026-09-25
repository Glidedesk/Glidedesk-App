// Bridge to the Rust side. All agent requests go through one validated command.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AgentStatus, Config, DeviceId, ImportPreview, NetInterface, NoticeView } from "./types";

type Req = Record<string, unknown> & { cmd: string };

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export async function agent<T = unknown>(req: Req): Promise<T> {
  if (!inTauri) throw new Error("Glidedesk UI must run inside the app");
  return invoke<T>("agent", { request: req });
}

export const api = {
  status: () => invoke<AgentStatus>("status"),
  getConfig: () => agent<Config>({ cmd: "get_config" }),
  setConfig: (config: Config) => agent<{ issues: string[] }>({ cmd: "set_config", config }),
  start: () => agent({ cmd: "start" }),
  stop: () => agent({ cmd: "stop" }),
  reload: () => agent({ cmd: "reload" }),
  quit: () => agent({ cmd: "quit" }),
  switchTo: (id: DeviceId) => agent({ cmd: "switch_to", id }),
  switchHome: () => agent({ cmd: "switch_home" }),
  setLocked: (locked: boolean) => agent({ cmd: "set_locked", locked }),
  reconnectAll: () => agent({ cmd: "reconnect_all" }),
  identify: () => agent({ cmd: "identify" }),
  disconnect: (id: DeviceId) => agent({ cmd: "disconnect", id }),
  setBlocked: (id: DeviceId, blocked: boolean) => agent({ cmd: "set_blocked", id, blocked }),
  forget: (id: DeviceId) => agent({ cmd: "forget", id }),
  wake: (id: DeviceId) => agent({ cmd: "wake", id }),
  interfaces: () => agent<NetInterface[]>({ cmd: "list_interfaces" }),
  resetConfig: () => agent({ cmd: "reset_config" }),
  /** Asks macOS from the app itself, so the prompt names Glidedesk. */
  requestPermissions: () => invoke<void>("request_permissions"),
  setServerPassword: (password: string) => agent({ cmd: "set_server_password", password }),
  selfTest: () => agent<Record<string, unknown>>({ cmd: "self_test" }),
  exportSettings: (layoutOnly: boolean) => invoke<boolean>("export_settings", { layoutOnly }),
  importSettings: () => invoke<ImportPreview | null>("import_settings"),
  openExternal: (target: "accessibility" | "input-monitoring" | "logs" | "uninstall") => invoke<void>("open_external", { target }),
};

export function onStatus(cb: (s: AgentStatus) => void): Promise<UnlistenFn> {
  return listen<AgentStatus>("agent-status", (e) => cb(e.payload));
}

export function onNotice(cb: (n: NoticeView) => void): Promise<UnlistenFn> {
  return listen<NoticeView>("agent-notice", (e) => cb(e.payload));
}

/** Settings changed outside this window (new client, reset, reload). */
export function onConfigChanged(cb: () => void): Promise<UnlistenFn> {
  return listen("agent-config", () => cb());
}

export function onNavigate(cb: (page: string) => void): Promise<UnlistenFn> {
  return listen<string>("navigate", (e) => cb(e.payload));
}

export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  return typeof e === "string" ? e : JSON.stringify(e);
}
