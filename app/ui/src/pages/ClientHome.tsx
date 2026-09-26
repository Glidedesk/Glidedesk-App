import { api } from "../lib/api";
import { useToast } from "../lib/toast";
import { Icon } from "../components/icons";
import { linkLabel } from "../lib/format";
import type { AgentStatus, Config, RemapPreset } from "../lib/types";
import { Badge, Button, Dot, NumberInput, Page, Row, Section, Select, Switch, TextInput } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;

const REMAP: Record<RemapPreset, string> = {
  none: "native keys",
  auto: "Cmd and Ctrl swapped between Mac and PC",
  "swap-ctrl-meta": "Cmd and Ctrl always swapped",
};

export function ClientHome({ status, config, update }: { status: AgentStatus; config: Config; update: Update }) {
  const c = status.client;
  const { run } = useToast();
  const tone = c?.state === "connected" ? "ok" : c?.state === "rejected" ? "bad" : c?.state === "searching" || c?.state === "connecting" ? "busy" : "muted";
  const cl = config.client;
  const applied = c?.applied ?? null;
  const fromServer = (v: string | null) => (v == null ? "Set by the server" : `Set by the server: ${v}`);
  const override = <K extends "mouse_speed" | "scroll_speed">(k: K, label: string) => (
    <Row
      label={label}
      hint={cl[k] == null ? fromServer(applied ? `${applied[k].toFixed(1)}×` : null) : "Overridden on this computer"}
    >
      <Switch label={`Override ${label}`} checked={cl[k] != null} onChange={(on) => update((x) => void (x.client[k] = on ? 1 : null), true)} />
      {cl[k] != null && <NumberInput label={label} value={cl[k] ?? 1} min={0.1} max={10} step={0.1} unit="×" onChange={(v) => update((x) => void (x.client[k] = v))} />}
    </Row>
  );
  return (
    <Page
      title="This computer"
      subtitle="Controlled by a Glidedesk server on your network."
      actions={
        <>
          <Button icon={<Icon.Refresh size={16} />} onClick={() => void run(() => api.reconnectAll())}>
            Reconnect
          </Button>
          <Button
            icon={status.running ? <Icon.Stop size={15} /> : <Icon.Play size={15} />}
            onClick={() => void run(() => (status.running ? api.stop() : api.start()))}
            variant={status.running ? "default" : "primary"}
          >
            {status.running ? "Stop" : "Start"}
          </Button>
        </>
      }
    >
      <div className="mb-6 rounded-2xl border border-line bg-panel p-4 sm:p-5">
        <div className="flex items-center gap-3">
          <Badge tone={tone}>
            <Dot tone={tone} /> {c ? linkLabel(c.state) : "Stopped"}
          </Badge>
          {c?.active && <Badge tone="accent">Cursor is here</Badge>}
        </div>
        <div className="mt-3 text-[18px] font-semibold">{c?.server_name ?? "No server yet"}</div>
        <div className="mt-1 text-muted">
          <span className="break-all">{c?.server_address ?? "—"}</span>
          {c?.latency_ms != null ? ` · ${c.latency_ms.toFixed(1)} ms` : ""}
          {c?.server_version ? ` · v${c.server_version}` : ""}
        </div>
        {c?.message && <div className="mt-2 text-muted">{c.message}</div>}
      </div>
      <Section title="Server">
        <Row label="Server address" hint="Leave empty to find the server automatically, or type its computer name (e.g. Studio-Mac) or IP address, optionally with :port.">
          <TextInput label="Server address" width="w-full sm:w-64" value={cl.server_address} placeholder="Automatic" onChange={(v) => update((x) => void (x.client.server_address = v.trim()))} />
        </Row>
        <Row label="Password" hint="Only if the server has a password (Network → Password on the server).">
          <TextInput type="password" width="w-full sm:w-64" autoComplete="current-password" label="Server password" value={cl.password} placeholder="None" onChange={(v) => update((x) => void (x.client.password = v))} />
        </Row>
      </Section>
      <Section
        title="Overrides"
        description="By default this computer uses what the server sets for it — pointer speed, scrolling and keys follow the server. Settings here win over the server's."
      >
        {override("mouse_speed", "Mouse speed")}
        {override("scroll_speed", "Scroll speed")}
        <Row
          label="Reverse scrolling"
          hint={
            cl.scroll_invert == null
              ? fromServer(applied ? (applied.scroll_invert ? "reversed" : "same direction as the server") : null)
              : "Overridden on this computer"
          }
        >
          <Select
            label="Reverse scrolling"
            value={cl.scroll_invert == null ? "server" : cl.scroll_invert ? "yes" : "no"}
            onChange={(v) => update((x) => void (x.client.scroll_invert = v === "server" ? null : v === "yes"), true)}
            options={[
              ["server", "Use server setting"],
              ["yes", "Reverse"],
              ["no", "Normal"],
            ]}
          />
        </Row>
        <Row label="Cmd / Ctrl" hint={cl.key_remap == null ? fromServer(applied ? REMAP[applied.key_remap] : null) : "Overridden on this computer"}>
          <Select<"server" | RemapPreset>
            label="Modifier mapping"
            value={cl.key_remap ?? "server"}
            onChange={(v) => update((x) => void (x.client.key_remap = v === "server" ? null : v), true)}
            options={[
              ["server", "Use server setting"],
              ["none", "Native (like a keyboard plugged into it)"],
              ["auto", "Swap Cmd and Ctrl between Mac and PC"],
              ["swap-ctrl-meta", "Always swap Cmd and Ctrl"],
            ]}
          />
        </Row>
        <Row label="Show a cursor without a mouse" hint="Windows hides the pointer when no mouse is plugged in (servers, VMs).">
          <Switch label="Draw cursor" checked={cl.draw_cursor} onChange={(v) => update((x) => void (x.client.draw_cursor = v), true)} />
        </Row>
        <Row label="Sync Caps / Num Lock lights">
          <Switch label="LED sync" checked={cl.led_sync} onChange={(v) => update((x) => void (x.client.led_sync = v), true)} />
        </Row>
      </Section>
    </Page>
  );
}
