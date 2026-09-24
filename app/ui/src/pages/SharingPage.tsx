import { bytes } from "../lib/format";
import type { AgentStatus, Config, TransferView } from "../lib/types";
import { Badge, Callout, Page, Row, Section, Select, Switch } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;

const SIZES: [string, string][] = [
  ["0", "Unlimited"],
  [String(10 << 20), "10 MB"],
  [String(100 << 20), "100 MB"],
  [String(1 << 30), "1 GB"],
  [String(10 * (1 << 30)), "10 GB"],
];

export function SharingPage({ status, config, update }: { status: AgentStatus; config: Config; update: Update }) {
  const isClient = status.role === "client";
  const s = config.server.sharing;
  return (
    <Page title="Clipboard & Files" subtitle="Copy on one computer, paste on another — text, images and files of any size.">
      <Callout tone="info">
        The clipboard follows the cursor: when you move to another computer, whatever you copied goes with you — text, images, and files or
        folders of any size. Files are checked and only appear on the clipboard once complete, so Ctrl+V / Cmd+V pastes them in Explorer or
        Finder. Cut files (Windows) are moved to the Recycle Bin / Trash on the original computer only after you paste them.
      </Callout>
      <Transfers list={status.server?.transfers ?? status.client?.transfers ?? []} />
      {isClient ? (
        <Section title="On this computer" description="Both sides must allow sharing. Turning it off here wins.">
          <Row label="Accept clipboard">
            <Switch label="Accept clipboard" checked={config.client.accept_clipboard} onChange={(v) => update((c) => void (c.client.accept_clipboard = v), true)} />
          </Row>
          <Row label="Accept files">
            <Switch label="Accept files" checked={config.client.accept_files} onChange={(v) => update((c) => void (c.client.accept_files = v), true)} />
          </Row>
        </Section>
      ) : (
        <>
          <Section title="Sharing">
            <Row label="Share clipboard" hint="Text, rich text and images.">
              <Switch label="Share clipboard" checked={s.clipboard} onChange={(v) => update((c) => void (c.server.sharing.clipboard = v), true)} />
            </Row>
            <Row label="Share files" hint="Copy or cut in Finder / Explorer, paste on the other computer.">
              <Switch label="Share files" checked={s.files} onChange={(v) => update((c) => void (c.server.sharing.files = v), true)} />
            </Row>
            <Row label="Direction">
              <Select
                label="Direction"
                value={s.direction}
                onChange={(v) => update((c) => void (c.server.sharing.direction = v), true)}
                options={[
                  ["both", "Both ways"],
                  ["to-clients", "Only from this computer to others"],
                  ["from-clients", "Only from others to this computer"],
                ]}
              />
            </Row>
          </Section>
          <Section title="Size">
            <Row label="Send immediately up to" hint="Smaller items are pushed right away for instant paste; bigger ones travel when you paste.">
              <Select
                label="Eager size"
                value={String(s.eager_bytes)}
                onChange={(v) => update((c) => void (c.server.sharing.eager_bytes = Number(v)), true)}
                options={[
                  [String(256 << 10), "256 KB"],
                  [String(1 << 20), "1 MB"],
                  [String(8 << 20), "8 MB"],
                ]}
              />
            </Row>
            <Row label="Largest clipboard item" hint={`Currently ${bytes(s.max_clipboard_bytes)}.`}>
              <Select label="Maximum" value={String(s.max_clipboard_bytes)} onChange={(v) => update((c) => void (c.server.sharing.max_clipboard_bytes = Number(v)), true)} options={SIZES} />
            </Row>
          </Section>
        </>
      )}
    </Page>
  );
}

function Transfers({ list }: { list: TransferView[] }) {
  if (list.length === 0) return null;
  return (
    <Section title="Transfers">
      {list.map((t) => {
        const pct = t.total > 0 ? Math.min(100, Math.round((t.done / t.total) * 100)) : t.state === "done" ? 100 : 0;
        return (
          <div key={t.id} className="px-4 py-3">
            <div className="flex items-center justify-between gap-3">
              <span className="truncate font-medium">
                {t.outgoing ? "→" : "←"} {t.name} <span className="text-muted">{t.outgoing ? `to ${t.peer}` : `from ${t.peer}`}</span>
              </span>
              <Badge tone={t.state === "failed" ? "bad" : t.state === "done" ? "ok" : "busy"}>
                {t.state === "active" ? `${pct}%` : t.state === "done" ? "Done" : "Failed"}
              </Badge>
            </div>
            <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-panel-2" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
              <div className={`h-full ${t.state === "failed" ? "bg-bad" : "bg-accent"}`} style={{ width: `${pct}%` }} />
            </div>
            <div className="mt-1 text-[12px] text-muted">
              {t.done === 0 ? "0 B" : bytes(t.done)} of {t.total === 0 ? "0 B" : bytes(t.total)}
              {t.error ? ` — ${t.error}` : ""}
            </div>
          </div>
        );
      })}
    </Section>
  );
}
