import { useMemo, useState } from "react";
import { api } from "../lib/api";
import { useToast } from "../lib/toast";
import { Icon } from "../components/icons";
import type { AgentStatus, Config, DeviceId } from "../lib/types";
import { Button, Callout, Page } from "../components/ui";
import { LayoutEditor, useBoxes } from "../layout/LayoutEditor";
import { LinkPanel } from "../layout/LinkPanel";
import { placeAll } from "../layout/geometry";

export function LayoutPage({ status, config, update }: { status: AgentStatus; config: Config; update: (m: (c: Config) => void, now?: boolean) => void }) {
  const [selected, setSelected] = useState<DeviceId | null>(null);
  const { run } = useToast();
  const boxes = useBoxes(status, config);
  const placement = useMemo(() => placeAll(config.device.id, boxes, config.layout.link), [config, boxes]);
  const server = status.server;
  const online = server?.clients.filter((c) => ["online", "degraded", "locked"].includes(c.state)).length ?? 0;
  const setLinks = (links: Config["layout"]["link"]) => update((c) => void (c.layout.link = links), true);
  const selectedClient = selected && selected !== config.device.id ? selected : null;

  return (
    <Page
      title="Layout"
      subtitle="Drag computers to the side of the screen they sit on. The cursor moves across that edge."
      actions={
        <>
          <Button icon={<Icon.Eye size={16} />} onClick={() => void run(() => api.identify())} disabled={!status.running}>
            Identify screens
          </Button>
          <Button
            icon={status.running ? <Icon.Stop size={15} /> : <Icon.Play size={15} />}
            onClick={() => void run(() => (status.running ? api.stop() : api.start()))}
            variant={status.running ? "default" : "primary"}
          >
            {status.running ? "Stop sharing" : "Start sharing"}
          </Button>
        </>
      }
    >
      {server?.warnings.map((w) => (
        <Callout key={w} tone="warn">
          {w}
        </Callout>
      ))}
      <div className="flex h-[calc(100%-8px)] min-h-[440px] gap-4">
        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <div className="min-h-0 flex-1">
            <LayoutEditor status={status} config={config} selected={selected} onSelect={setSelected} onLinks={setLinks} />
          </div>
          <div className="flex flex-wrap items-center gap-x-5 gap-y-1 text-muted">
            <span>
              {online} of {server?.clients.length ?? 0} computers online
            </span>
            <span>Clipboard {status.clipboard ? "on" : "off"}</span>
            <span>Files {status.files ? "on" : "off"}</span>
            {server?.locked && <span className="text-warn">Cursor locked to this screen</span>}
            <span className="ml-auto">Tip: select a computer and use the arrow keys to move it.</span>
          </div>
        </div>
        <aside className="w-[300px] shrink-0 overflow-y-auto rounded-2xl border border-line bg-panel p-4" aria-label="Selected computer">
          {selectedClient ? (
            <>
              <h2 className="mb-3 text-[15px] font-semibold">{boxes.get(selectedClient)?.name}</h2>
              <LinkPanel id={selectedClient} config={config} boxes={boxes} placement={placement} onLinks={setLinks} />
            </>
          ) : (
            <div className="space-y-3 text-muted">
              <p className="font-medium text-fg">Select a computer</p>
              <p>New computers appear automatically when Glidedesk runs on them in Client mode, and are placed on a free side.</p>
              <p>With several monitors, choose which monitor edges lead to each computer — one, several, or all.</p>
            </div>
          )}
        </aside>
      </div>
    </Page>
  );
}
