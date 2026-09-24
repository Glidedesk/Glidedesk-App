import { useState } from "react";
import { api, errorText } from "../lib/api";
import { useConfig } from "../lib/config";
import type { AgentStatus, Config, Role } from "../lib/types";
import { Button, Callout, TextInput } from "../components/ui";

export function Wizard({ status, config, onDone }: { status: AgentStatus; config: Config; onDone: () => void }) {
  const [step, setStep] = useState(0);
  const [role, setRole] = useState<Role>("server");
  const [address, setAddress] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const { commit } = useConfig();
  const mac = status.platform === "macos";
  const perms = status.permissions;
  const finish = async () => {
    try {
      const next = structuredClone(config);
      next.device.role = role;
      if (role === "client") next.client.server_address = address.trim();
      await commit(next);
      onDone();
    } catch (e) {
      setErr(errorText(e));
    }
  };
  const steps = [
    <div key="role" className="space-y-4">
      <h2 className="text-[22px] font-semibold">How will this computer be used?</h2>
      <div className="grid grid-cols-2 gap-4">
        {(
          [
            ["server", "Server", "Its keyboard and mouse control the other computers."],
            ["client", "Client", "It is controlled from another computer."],
          ] as const
        ).map(([r, t, d]) => (
          <button
            type="button"
            key={r}
            aria-pressed={role === r}
            onClick={() => setRole(r)}
            className={`rounded-2xl border p-5 text-left transition ${role === r ? "border-accent bg-accent-soft" : "border-line bg-panel hover:bg-panel-2"}`}
          >
            <div className="text-[16px] font-semibold">{t}</div>
            <div className="mt-1 text-muted">{d}</div>
          </button>
        ))}
      </div>
    </div>,
    <div key="perm" className="space-y-4">
      <h2 className="text-[22px] font-semibold">{mac ? "Allow Glidedesk to use the keyboard and mouse" : "Almost there"}</h2>
      {mac ? (
        <>
          <p className="text-muted">macOS asks once. Turn Glidedesk on in both lists, then come back — this page updates by itself.</p>
          <div className="space-y-2">
            <div className="flex items-center justify-between rounded-xl border border-line bg-panel px-4 py-3">
              <span>Accessibility {perms.accessibility ? "✓" : ""}</span>
              <Button onClick={() => void api.openExternal("accessibility").catch((e) => setErr(errorText(e)))}>Open</Button>
            </div>
            {role === "server" && (
              <div className="flex items-center justify-between rounded-xl border border-line bg-panel px-4 py-3">
                <span>Input Monitoring {perms.input_monitoring ? "✓" : ""}</span>
                <Button onClick={() => void api.openExternal("input-monitoring").catch((e) => setErr(errorText(e)))}>Open</Button>
              </div>
            )}
            <Button onClick={() => void api.requestPermissions().catch((e) => setErr(errorText(e)))}>Ask macOS again</Button>
          </div>
        </>
      ) : (
        <p className="text-muted">Windows may ask to allow Glidedesk through the firewall on private networks — choose Allow.</p>
      )}
    </div>,
    <div key="net" className="space-y-4">
      {role === "server" ? (
        <>
          <h2 className="text-[22px] font-semibold">Ready to share</h2>
          <p className="text-muted">
            Glidedesk will listen on all networks. You can pick exact interfaces or IP addresses later in Network. Install Glidedesk on your
            other computers and choose Client — they appear here automatically.
          </p>
        </>
      ) : (
        <>
          <h2 className="text-[22px] font-semibold">Find the server</h2>
          <p className="text-muted">Leave empty to find it automatically on this network, or type its name or IP address.</p>
          <TextInput label="Server address" value={address} placeholder="Automatic" onChange={setAddress} width="w-80" />
        </>
      )}
    </div>,
  ];
  return (
    <div className="flex h-full items-center justify-center p-8">
      <div className="w-[620px] max-w-full rounded-3xl border border-line bg-panel p-8 shadow-sm">
        <div className="mb-6 flex gap-1.5" aria-label={`Step ${step + 1} of ${steps.length}`}>
          {steps.map((_, i) => (
            <span key={i} className={`h-1.5 flex-1 rounded-full ${i <= step ? "bg-accent" : "bg-line"}`} />
          ))}
        </div>
        {err && <Callout tone="bad">{err}</Callout>}
        {steps[step]}
        <div className="mt-8 flex justify-between">
          <Button variant="ghost" disabled={step === 0} onClick={() => setStep(step - 1)}>
            Back
          </Button>
          {step < steps.length - 1 ? (
            <Button variant="primary" onClick={() => setStep(step + 1)}>
              Continue
            </Button>
          ) : (
            <Button variant="primary" onClick={() => void finish()}>
              Finish
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
