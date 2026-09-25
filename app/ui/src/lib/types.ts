// Mirrors of the Rust types that cross the agent ⇄ UI boundary (serde JSON).

export type DeviceId = string;
export type Role = "unset" | "server" | "client";
export type Platform = "macos" | "windows" | "linux" | "other";
export type Side = "left" | "right" | "top" | "bottom";

export interface Rect { x: number; y: number; w: number; h: number }
export interface MonitorInfo { id: string; name: string; bounds: Rect; scale: number; primary: boolean }

export type HealthState = "never-connected" | "connecting" | "online" | "degraded" | "locked" | "offline";
export type LinkState = "stopped" | "searching" | "connecting" | "connected" | "rejected" | "offline";

export interface ClientStatus { screen_locked: boolean; going_to_sleep: boolean; session_inactive: boolean; permission_missing: boolean }

export interface ClientView {
  id: DeviceId; name: string; platform: Platform | null; state: HealthState; latency_ms: number | null;
  last_seen: number | null; address: string | null; app_version: string | null; monitors: MonitorInfo[];
  status: ClientStatus; blocked: boolean;
}

export interface TransferView {
  id: number; peer: string; outgoing: boolean; name: string; done: number; total: number;
  state: "active" | "done" | "failed"; error: string | null;
}

export interface MachineView { id: DeviceId | null; name: string; monitors: MonitorInfo[] }

export interface ServerView {
  running: boolean; bind: { addr: string; error: string | null }[]; fingerprint: string; local: MachineView;
  clients: ClientView[]; focus: DeviceId | null; locked: boolean; warnings: string[]; transfers: TransferView[];
}

export interface ClientSideView {
  state: LinkState; server_id: DeviceId | null; server_name: string | null; server_address: string | null;
  server_version: string | null; latency_ms: number | null; active: boolean; clipboard: boolean; files: boolean;
  message: string | null; local: MachineView; transfers: TransferView[];
}

export interface AgentStatus {
  version: string; platform: Platform | null; role: Role; running: boolean; error: string | null;
  server: ServerView | null; client: ClientSideView | null;
  permissions: { accessibility: boolean; input_monitoring: boolean };
  config_issues: string[]; config_read_only: boolean; config_dir: string; log_dir: string;
  clipboard: boolean; files: boolean; notifications: boolean; start_at_login: boolean; device_name: string;
}

// ---- config.toml -----------------------------------------------------------

export type HandoverMode = "all" | "single" | "selected";
export interface MonitorSelection { mode: HandoverMode; monitors?: string[] }
export type Mapping = "continuous" | "per-monitor";
export interface Span { start: number; end: number }
export interface LinkSpec {
  from: DeviceId; to: DeviceId; side: Side; handover: MonitorSelection; mapping: Mapping;
  from_span: Span; entry: MonitorSelection; to_span: Span;
}
export type RemapPreset = "auto" | "none" | "swap-ctrl-meta";
export type ModifierKey = "shift" | "ctrl" | "alt" | "meta";

export interface ClientEntry {
  id: DeviceId; name: string; mouse_speed: number; scroll_speed: number; scroll_invert: boolean;
  key_remap: RemapPreset; clipboard: boolean; files: boolean; relative_mouse: boolean; notifications: boolean;
  blocked: boolean; mac_address: string;
}

export interface Config {
  schema_version: number;
  device: { id: DeviceId; name: string; role: Role };
  general: {
    start_at_login: boolean; notifications: boolean; theme: "system" | "light" | "dark"; language: string;
    log_level: "error" | "warn" | "info" | "debug" | "trace"; confirm_quit: boolean; auto_update: boolean;
  };
  server: {
    network: {
      mode: "all" | "interfaces" | "addresses"; interfaces: string[]; addresses: string[]; port: number;
      same_subnet_only: boolean; allow_list: string[]; block_list: string[]; discovery: boolean;
      /** Salted Argon2id key of the server password (never the password itself). */
      password: { salt: string; key: string } | null;
    };
    health: { interval_ms: number; miss_threshold: number; degraded_latency_ms: number; idle_timeout_ms: number };
    switching: {
      delay_ms: number; double_tap_ms: number; modifier: ModifierKey | null; dead_corner_px: number;
      wrap: boolean; block_fullscreen: boolean;
    };
    fullscreen_allow: string[];
    hotkeys: {
      lock_cursor: string; switch_home: string; switch_next: string; switch_previous: string;
      reconnect_all: string; identify: string; toggle_sharing: string;
    };
    sharing: { clipboard: boolean; files: boolean; direction: "both" | "to-clients" | "from-clients"; eager_bytes: number; max_clipboard_bytes: number };
    visuals: { cursor_locator: boolean; identify_seconds: number; notify_connect: boolean; notify_offline: boolean };
    clients: ClientEntry[];
  };
  client: {
    server_address: string; password: string; interface: string; receive_dir: string | null; accept_clipboard: boolean; accept_files: boolean;
    mouse_speed: number | null; scroll_speed: number | null; scroll_invert: boolean | null; key_remap: RemapPreset | null;
    draw_cursor: boolean; led_sync: boolean;
  };
  layout: { tiles: { machine: DeviceId; x: number; y: number }[]; link: LinkSpec[] };
}

export interface NetInterface {
  name: string; friendly_name: string; kind: "ethernet" | "wifi" | "vpn" | "loopback" | "virtual" | "other";
  up: boolean; addrs: { ip: string; prefix: number; scope_id: number }[]; mac: string | null;
}

export type NoticeView =
  | { kind: "new-client"; id: DeviceId; name: string; address: string }
  | { kind: "client-online"; id: DeviceId; name: string }
  | { kind: "client-offline"; id: DeviceId; name: string }
  | { kind: "server-connected"; name: string }
  | { kind: "identify"; label: string }
  | { kind: "error"; message: string };

export interface ImportPreview { config: Config; layout_only: boolean; issues: string[]; changed_sections: string[] }
