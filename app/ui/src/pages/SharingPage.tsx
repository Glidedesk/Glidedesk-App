import { bytes } from "../lib/format";
import type { ActivityView, AgentStatus, Config, TransferView } from "../lib/types";
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
        The clipboard follows the cursor. Text and images go with you when you move to another computer. Copied files and folders (any
        size) are only <b>offered</b>: nothing is copied until you paste them in a folder there — then they download and your file manager
        pastes them. Files that were already on the clipboard when Nexpingdesk started, or were copied over an hour ago, are not offered.
        Everything that happens is listed under Activity below.
      </Callout>
      <Transfers list={status.server?.transfers ?? status.client?.transfers ?? []} />
      <Activity list={status.server?.activity ?? status.client?.activity ?? []} />
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
            <Row label="Largest clipboard item" hint={`Text and images bigger than this stay on their computer. Currently ${bytes(s.max_clipboard_bytes)}.`}>
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
              <span className="min-w-0 truncate font-medium">
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
            {t.detail && <div className="mt-0.5 break-all text-[12px] text-muted">{t.detail}</div>}
          </div>
        );
      })}
    </Section>
  );
}

function Activity({ list }: { list: ActivityView[] }) {
  const tone = { info: "bg-accent", ok: "bg-ok", bad: "bg-bad" } as const;
  return (
    <Section title="Activity" description="What the clipboard and files did recently, newest first — what moved, where and why.">
      {list.length === 0 ? (
        <div className="px-4 py-3 text-muted">Nothing yet. Copy something and move the cursor to another computer.</div>
      ) : (
        <ul aria-label="Clipboard and files activity">
          {list.map((a, i) => (
            <li key={`${a.at}-${i}`} className="flex gap-3 border-t border-line px-4 py-2.5 first:border-t-0">
              <span aria-hidden className={`mt-[7px] h-1.5 w-1.5 shrink-0 rounded-full ${tone[a.tone]}`} />
              <span className="min-w-0 flex-1 break-words">{a.text}</span>
              <time className="shrink-0 text-[12px] tabular-nums text-muted" dateTime={new Date(a.at * 1000).toISOString()}>
                {new Date(a.at * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
              </time>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}
