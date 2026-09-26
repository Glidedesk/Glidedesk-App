import { useState } from "react";
import { api } from "../lib/api";
import { useConfig } from "../lib/config";
import { useToast } from "../lib/toast";
import type { AgentStatus, Config, ImportPreview } from "../lib/types";
import { Icon } from "../components/icons";
import { Badge, Button, Callout, Modal, Page, Row, Section, Select, Spinner } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;
type Report = Record<string, unknown>;

function Check({ ok, label, detail }: { ok: boolean; label: string; detail?: string }) {
  return (
    <div className="flex items-start gap-3 py-2">
      <span className={`mt-0.5 ${ok ? "text-ok" : "text-bad"}`}>{ok ? <Icon.Check size={18} /> : <Icon.Error size={18} />}</span>
      <div className="min-w-0">
        <div className="font-medium">{label}</div>
        {detail && <div className="text-[12.5px] break-words text-muted">{detail}</div>}
      </div>
    </div>
  );
}

function SelfTestView({ r }: { r: Report }) {
  const perms = (r.permissions ?? {}) as { accessibility?: boolean; input_monitoring?: boolean };
  const capture = (r.capture ?? {}) as { ok?: boolean; error?: string; note?: string };
  const inject = (r.injector ?? {}) as { ok?: boolean; error?: string };
  const mons = r.monitors as { Ok?: unknown[]; Err?: string } | undefined;
  const ifaces = (r.interfaces ?? []) as { kind: string }[];
  const lat = r.loopback_quic_ms as { p50?: number; p95?: number } | null;
  return (
    <div className="divide-y divide-line">
      <Check ok={!!perms.accessibility} label="Permission to use the keyboard and mouse" detail={perms.accessibility ? "Allowed" : "Allow Glidedesk under Accessibility"} />
      <Check ok={!!capture.ok} label="Read this computer's keyboard and mouse" detail={capture.error ?? capture.note} />
      <Check ok={!!inject.ok} label="Type and move the pointer here" detail={inject.error} />
      <Check ok={!!mons?.Ok} label="Screens" detail={mons?.Ok ? `${mons.Ok.length} found` : mons?.Err} />
      <Check ok={ifaces.some((i) => i.kind !== "loopback")} label="Network" detail={`${ifaces.filter((i) => i.kind !== "loopback").length} interfaces`} />
      <Check
        ok={!!lat?.p95}
        label="Encrypted connection speed"
        detail={lat?.p50 != null ? `${lat.p50.toFixed(2)} ms typical, ${lat.p95?.toFixed(2) ?? "?"} ms worst (local)` : "not measured"}
      />
      <details className="py-2">
        <summary className="cursor-pointer text-muted">Full report</summary>
        <pre className="mt-2 max-h-64 overflow-auto rounded-lg bg-panel-2 p-3 font-mono text-[11.5px] select-text">{JSON.stringify(r, null, 2)}</pre>
      </details>
    </div>
  );
}

export function AdvancedPage({ status, config, update, reload }: { status: AgentStatus; config: Config; update: Update; reload: () => void }) {
  const { run } = useToast();
  const { commit } = useConfig();
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [report, setReport] = useState<Report | null>(null);
  const [testing, setTesting] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);

  const selfTest = async () => {
    setTesting(true);
    try {
      await run(async () => setReport(await api.selfTest()));
    } finally {
      setTesting(false);
    }
  };

  return (
    <Page title="Advanced" subtitle="Settings files, diagnostics and logs.">
      {status.config_issues.length > 0 && (
        <Callout tone="warn">
          <div className="font-medium">Some settings were repaired when loading:</div>
          <ul className="mt-1 list-disc pl-5">
            {status.config_issues.map((i) => (
              <li key={i}>{i}</li>
            ))}
          </ul>
        </Callout>
      )}
      <Section title="Settings file" description={`Stored privately in ${status.config_dir}`}>
        <Row label="Export all settings" hint="Everything except this computer's identity.">
          <Button onClick={() => void run(async () => { if (!(await api.exportSettings(false))) throw new Error("Export cancelled"); }, "Settings exported")}>Export…</Button>
        </Row>
        <Row label="Export layout only" hint="Screen arrangement and per-computer settings.">
          <Button onClick={() => void run(async () => { if (!(await api.exportSettings(true))) throw new Error("Export cancelled"); }, "Layout exported")}>Export…</Button>
        </Row>
        <Row label="Import" hint="Shows what will change before applying.">
          <Button onClick={() => void run(async () => setPreview(await api.importSettings()))}>Import…</Button>
        </Row>
        <Row label="Reset to defaults" hint="A backup of the current file is kept.">
          <Button variant="danger" onClick={() => setConfirmReset(true)}>
            Reset…
          </Button>
        </Row>
      </Section>
      <Section title="Diagnostics">
        <Row label="Self-test" hint="Checks permissions, input, screens, network and encrypted connection speed.">
          <Button icon={testing ? <Spinner size={14} /> : undefined} disabled={testing} onClick={() => void selfTest()}>
            {testing ? "Testing…" : "Run self-test"}
          </Button>
        </Row>
        <Row label="Log files" hint={status.log_dir}>
          <Button onClick={() => void run(() => api.openExternal("logs"))}>Open folder</Button>
        </Row>
        <Row label="Log detail">
          <Select
            label="Log level"
            value={config.general.log_level}
            onChange={(v) => update((c) => void (c.general.log_level = v), true)}
            options={[
              ["error", "Errors only"],
              ["warn", "Warnings"],
              ["info", "Normal"],
              ["debug", "Detailed"],
              ["trace", "Everything (large)"],
            ]}
          />
        </Row>
        {status.server && (
          <Row label="Session fingerprint" hint="Changes every time the server starts (traffic is always encrypted).">
            <span className="font-mono text-muted">{status.server.fingerprint}</span>
          </Row>
        )}
        <Row label="Version">
          <span className="text-muted">Glidedesk {status.version}</span>
        </Row>
      </Section>
      {status.platform === "macos" && (
        <Section title="macOS permissions">
          <Row label="Accessibility" hint="Required: lets Glidedesk read and control this Mac's keyboard and mouse.">
            {status.permissions.accessibility ? (
              <Badge tone="ok">Allowed</Badge>
            ) : (
              <>
                <Button variant="primary" onClick={() => void run(() => api.requestPermissions())}>
                  Allow…
                </Button>
                <Button onClick={() => void run(() => api.openExternal("accessibility"))}>Open settings</Button>
              </>
            )}
          </Row>
          <Row label="Input Monitoring" hint="Not required. Only turn it on if typing doesn't reach your other computers.">
            {status.permissions.input_monitoring ? <Badge tone="ok">Allowed</Badge> : <Button variant="ghost" onClick={() => void run(() => api.openExternal("input-monitoring"))}>Open settings</Button>}
          </Row>
        </Section>
      )}
      <Section title="Uninstall">
        <Row label="Uninstall Glidedesk" hint="Asks whether to keep your settings for a future install.">
          <Button variant="danger" onClick={() => void run(() => api.openExternal("uninstall"))}>
            Uninstall…
          </Button>
        </Row>
      </Section>

      <Modal
        open={!!preview}
        onClose={() => setPreview(null)}
        title="Import settings"
        footer={
          <>
            <Button onClick={() => setPreview(null)}>Cancel</Button>
            <Button
              variant="primary"
              onClick={() =>
                void run(async () => {
                  if (preview) await commit(preview.config);
                  setPreview(null);
                }, "Settings imported")
              }
            >
              Apply
            </Button>
          </>
        }
      >
        {preview && (
          <div className="space-y-2">
            <p>{preview.layout_only ? "Layout-only file." : "Full settings file."} This computer's identity and network selection are kept.</p>
            <p>Changes: {preview.changed_sections.length ? preview.changed_sections.join(", ") : "none"}.</p>
            {preview.issues.length > 0 && (
              <Callout tone="warn">
                Repaired values:
                <ul className="list-disc pl-5">
                  {preview.issues.map((i) => (
                    <li key={i}>{i}</li>
                  ))}
                </ul>
              </Callout>
            )}
          </div>
        )}
      </Modal>
      <Modal
        open={confirmReset}
        onClose={() => setConfirmReset(false)}
        title="Reset all settings?"
        footer={
          <>
            <Button onClick={() => setConfirmReset(false)}>Cancel</Button>
            <Button
              variant="danger"
              onClick={() =>
                void run(async () => {
                  await api.resetConfig();
                  setConfirmReset(false);
                  reload();
                }, "Settings reset — the previous file was backed up")
              }
            >
              Reset
            </Button>
          </>
        }
      >
        <p>Layout, computers and preferences go back to defaults. The current file is saved as a backup first.</p>
      </Modal>
      <Modal open={!!report} onClose={() => setReport(null)} title="Self-test" footer={<Button onClick={() => setReport(null)}>Close</Button>}>
        {report && <SelfTestView r={report} />}
      </Modal>
    </Page>
  );
}
