import { useState } from "react";
import { api } from "../lib/api";
import { useToast } from "../lib/toast";
import { Icon, PlatformIcon } from "../components/icons";
import { ago, healthLabel, healthTone } from "../lib/format";
import type { AgentStatus, ClientEntry, Config, DeviceId, RemapPreset } from "../lib/types";
import { Badge, Button, Card, Dot, EmptyState, Modal, NumberInput, Page, Row, Section, Select, Switch, TextInput } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;

function ClientSettings({ entry, update }: { entry: ClientEntry; update: Update }) {
  const set = <K extends keyof ClientEntry>(k: K, v: ClientEntry[K]) =>
    update((c) => {
      const e = c.server.clients.find((x) => x.id === entry.id);
      if (e) e[k] = v;
    });
  const macOk = entry.mac_address === "" || /^([0-9a-fA-F]{2}[:-]){5}[0-9a-fA-F]{2}$/.test(entry.mac_address);
  return (
    <>
      <Section title="Pointer">
        <Row label="Mouse speed" hint="Multiplies the automatic speed matching between screens.">
          <NumberInput label="Mouse speed" value={entry.mouse_speed} min={0.1} max={10} step={0.1} unit="×" onChange={(v) => set("mouse_speed", v)} />
        </Row>
        <Row label="Scroll speed">
          <NumberInput label="Scroll speed" value={entry.scroll_speed} min={0.1} max={10} step={0.1} unit="×" onChange={(v) => set("scroll_speed", v)} />
        </Row>
        <Row label="Reverse scrolling" hint="Useful between a Mac with natural scrolling and a PC.">
          <Switch label="Reverse scrolling" checked={entry.scroll_invert} onChange={(v) => set("scroll_invert", v)} />
        </Row>
        <Row label="Relative mouse (games)" hint="Sends movement instead of positions. Use for full-screen games.">
          <Switch label="Relative mouse" checked={entry.relative_mouse} onChange={(v) => set("relative_mouse", v)} />
        </Row>
      </Section>
      <Section title="Keyboard">
        <Row label="Cmd / Ctrl" hint="Auto swaps Cmd and Ctrl between a Mac and a PC so shortcuts keep working.">
          <Select<RemapPreset>
            label="Modifier mapping"
            value={entry.key_remap}
            onChange={(v) => set("key_remap", v)}
            options={[
              ["auto", "Automatic"],
              ["none", "Keep as is"],
              ["swap-ctrl-meta", "Always swap"],
            ]}
          />
        </Row>
      </Section>
      <Section title="Sharing">
        <Row label="Clipboard">
          <Switch label="Clipboard" checked={entry.clipboard} onChange={(v) => set("clipboard", v)} />
        </Row>
        <Row label="Files">
          <Switch label="Files" checked={entry.files} onChange={(v) => set("files", v)} />
        </Row>
        <Row label="Notifications" hint="Online / offline messages for this computer.">
          <Switch label="Notifications" checked={entry.notifications} onChange={(v) => set("notifications", v)} />
        </Row>
      </Section>
      <Section title="Wake-on-LAN">
        <Row label="MAC address" hint="Lets you wake this computer from the tray when it is offline.">
          <TextInput label="MAC address" value={entry.mac_address} placeholder="aa:bb:cc:dd:ee:ff" width="w-48" invalid={!macOk} onChange={(v) => set("mac_address", v.trim())} />
        </Row>
      </Section>
    </>
  );
}

export function ComputersPage({ status, config, update }: { status: AgentStatus; config: Config; update: Update }) {
  const [open, setOpen] = useState<DeviceId | null>(null);
  const { run } = useToast();
  const views = status.server?.clients ?? [];
  const entry = config.server.clients.find((c) => c.id === open);

  return (
    <Page title="Computers" subtitle="Every computer this server has seen. Offline ones stay here with their settings.">
      {config.server.clients.length === 0 ? (
        <EmptyState icon={<Icon.Computers size={24} />} title="No computers yet">
          Install Glidedesk on another computer and choose <b>Client</b>. It appears here automatically and is placed on a free side of
          this screen.
        </EmptyState>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
          {config.server.clients.map((c) => {
            const v = views.find((x) => x.id === c.id);
            const state = v?.state ?? "never-connected";
            const online = ["online", "degraded", "locked"].includes(state);
            const focused = status.server?.focus === c.id;
            return (
              <Card key={c.id} className={`p-4 transition ${focused ? "ring-2 ring-accent" : ""}`}>
                <div className="flex items-start gap-3">
                  <div className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-xl ${online ? "bg-accent-soft text-accent" : "bg-panel-2 text-muted"}`}>
                    <PlatformIcon platform={v?.platform} size={20} />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-[15px] font-semibold">{v?.name || c.name}</span>
                      {c.blocked && <Badge tone="bad">Blocked</Badge>}
                    </div>
                    <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12.5px] text-muted">
                      <Badge tone={healthTone(state)}>
                        <Dot tone={healthTone(state)} /> {healthLabel(state)}
                      </Badge>
                      {online && v?.latency_ms != null && <span className="tabular-nums">{v.latency_ms.toFixed(1)} ms</span>}
                      {!online && <span>Last seen {ago(v?.last_seen ?? null)}</span>}
                    </div>
                  </div>
                </div>
                <div className="mt-3 grid grid-cols-2 gap-x-3 gap-y-1 text-[12.5px]">
                  <span className="text-muted">Address</span>
                  <span className="truncate text-right font-mono">{v?.address ?? "—"}</span>
                  <span className="text-muted">Version</span>
                  <span className="text-right">{v?.app_version ? `v${v.app_version}` : "—"}</span>
                  <span className="text-muted">Screens</span>
                  <span className="text-right">{v?.monitors.length || "—"}</span>
                </div>
                <div className="mt-4 flex flex-wrap justify-end gap-2">
                  {online && !focused && (
                    <Button variant="ghost" icon={<Icon.Eye size={16} />} onClick={() => void run(() => api.switchTo(c.id))}>
                      Go there
                    </Button>
                  )}
                  {!online && c.mac_address && (
                    <Button variant="ghost" icon={<Icon.Power size={16} />} onClick={() => void run(() => api.wake(c.id), `Wake-up sent to ${c.name}`)}>
                      Wake
                    </Button>
                  )}
                  <Button icon={<Icon.Settings size={16} />} onClick={() => setOpen(c.id)}>
                    Settings
                  </Button>
                </div>
              </Card>
            );
          })}
        </div>
      )}
      <Modal
        open={!!entry}
        onClose={() => setOpen(null)}
        title={`${entry?.name ?? ""} — settings`}
        footer={
          entry && (
            <>
              <Button variant="danger" onClick={() => void run(() => api.forget(entry.id), `${entry.name} removed`).then((ok) => ok && setOpen(null))}>
                Forget
              </Button>
              <Button variant="danger" onClick={() => void run(() => api.setBlocked(entry.id, !entry.blocked))}>
                {entry.blocked ? "Unblock" : "Block"}
              </Button>
              <Button onClick={() => void run(() => api.disconnect(entry.id))}>Disconnect</Button>
              <Button variant="primary" onClick={() => setOpen(null)}>
                Done
              </Button>
            </>
          )
        }
      >
        {entry && <ClientSettings entry={entry} update={update} />}
      </Modal>
    </Page>
  );
}
