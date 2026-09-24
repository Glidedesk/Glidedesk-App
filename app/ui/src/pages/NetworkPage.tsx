import { useEffect, useState } from "react";
import { api, errorText } from "../lib/api";
import type { AgentStatus, Config, NetInterface } from "../lib/types";
import { Badge, Button, Callout, NumberInput, Page, Row, Section, Switch } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;
const KIND = { ethernet: "Ethernet", wifi: "Wi-Fi", vpn: "VPN", loopback: "Loopback", virtual: "Virtual", other: "Other" } as const;
const CIDR = /^(\d{1,3}(\.\d{1,3}){3}|[0-9a-f:]+)(\/\d{1,3})?$/i;

function ListEditor({ label, values, onChange }: { label: string; values: string[]; onChange: (v: string[]) => void }) {
  const [text, setText] = useState(values.join("\n"));
  useEffect(() => setText(values.join("\n")), [values]);
  const lines = text.split("\n").map((l) => l.trim()).filter(Boolean);
  const bad = lines.filter((l) => !CIDR.test(l));
  return (
    <div className="w-72">
      <textarea
        aria-label={label}
        className={`h-20 w-full rounded-lg border bg-panel px-2 py-1 font-mono text-[12.5px] ${bad.length ? "border-bad" : "border-line"}`}
        placeholder="192.168.1.0/24"
        value={text}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => bad.length === 0 && onChange(lines)}
      />
      {bad.length > 0 && <div className="text-[12px] text-bad">Not an address or range: {bad.join(", ")}</div>}
    </div>
  );
}

export function NetworkPage({ status, config, update }: { status: AgentStatus; config: Config; update: Update }) {
  const [ifaces, setIfaces] = useState<NetInterface[]>([]);
  const [error, setError] = useState<string | null>(null);
  const net = config.server.network;
  const refresh = () => api.interfaces().then(setIfaces, (e) => setError(errorText(e)));
  useEffect(() => {
    void refresh();
  }, []);
  const isClient = status.role === "client";

  if (isClient) {
    return (
      <Page title="Network" subtitle="How this computer reaches the server.">
        <Section title="Interface" description="Leave on Any unless the server must be reached through one specific network.">
          <Row label="Use interface">
            <select
              aria-label="Interface"
              className="rounded-lg border border-line bg-panel px-2 py-1"
              value={config.client.interface}
              onChange={(e) => update((c) => void (c.client.interface = e.target.value), true)}
            >
              <option value="">Any</option>
              {ifaces
                .filter((i) => i.kind !== "loopback")
                .map((i) => (
                  <option key={i.name} value={i.name}>
                    {i.friendly_name} ({i.addrs.map((a) => a.ip).join(", ")})
                  </option>
                ))}
            </select>
          </Row>
        </Section>
      </Page>
    );
  }

  const toggle = (list: string[], v: string, on: boolean) => (on ? [...new Set([...list, v])] : list.filter((x) => x !== v));
  return (
    <Page title="Network" subtitle="Where this server listens for its computers." actions={<Button onClick={() => void refresh()}>Refresh</Button>}>
      {error && <Callout tone="bad">{error}</Callout>}
      <Section title="Listen on">
        {(
          [
            ["all", "All networks", "Every interface, including ones connected later."],
            ["interfaces", "Selected interfaces", "Every address of the ticked interfaces (follows DHCP changes)."],
            ["addresses", "Selected IP addresses", "Only the exact addresses ticked below."],
          ] as const
        ).map(([mode, label, hint]) => (
          <label key={mode} className="flex cursor-pointer items-start gap-3 px-4 py-3">
            <input type="radio" name="bind" className="mt-1" checked={net.mode === mode} onChange={() => update((c) => void (c.server.network.mode = mode), true)} />
            <span>
              <span className="block font-medium">{label}</span>
              <span className="text-[12.5px] text-muted">{hint}</span>
            </span>
          </label>
        ))}
      </Section>
      {net.mode !== "all" && (
        <Section title={net.mode === "interfaces" ? "Interfaces" : "Addresses"}>
          {ifaces
            .filter((i) => i.kind !== "loopback")
            .map((i) => (
              <div key={i.name} className="px-4 py-2.5">
                <label className="flex items-center gap-2 font-medium">
                  {net.mode === "interfaces" && (
                    <input
                      type="checkbox"
                      checked={net.interfaces.includes(i.name)}
                      onChange={(e) => update((c) => void (c.server.network.interfaces = toggle(c.server.network.interfaces, i.name, e.target.checked)), true)}
                    />
                  )}
                  {i.friendly_name} <Badge tone="muted">{KIND[i.kind]}</Badge>
                  {!i.up && <Badge tone="warn">down</Badge>}
                </label>
                <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 pl-6 text-muted">
                  {i.addrs.map((a) =>
                    net.mode === "addresses" ? (
                      <label key={a.ip} className="flex items-center gap-1.5 text-fg">
                        <input
                          type="checkbox"
                          checked={net.addresses.includes(a.ip)}
                          onChange={(e) => update((c) => void (c.server.network.addresses = toggle(c.server.network.addresses, a.ip, e.target.checked)), true)}
                        />
                        <span className="font-mono text-[12.5px]">{a.ip}</span>
                      </label>
                    ) : (
                      <span key={a.ip} className="font-mono text-[12.5px]">
                        {a.ip}
                      </span>
                    ),
                  )}
                </div>
              </div>
            ))}
        </Section>
      )}
      <Section title="Status">
        {(status.server?.bind ?? []).map((b) => (
          <Row key={b.addr} label={<span className="font-mono">{b.addr}</span>}>
            {b.error ? <Badge tone="bad">{b.error}</Badge> : <Badge tone="ok">listening</Badge>}
          </Row>
        ))}
        {!status.server && <Row label="Not listening" hint="Sharing is stopped or this computer is not the server." />}
      </Section>
      <Section title="Port & discovery">
        <Row label="Port" hint="UDP. Change only if another program uses it.">
          <NumberInput label="Port" value={net.port} min={1024} max={65535} width="w-28" onChange={(v) => update((c) => void (c.server.network.port = v))} />
        </Row>
        <Row label="Announce on the network" hint="Lets clients find this server automatically (mDNS), only on the interfaces above.">
          <Switch label="Discovery" checked={net.discovery} onChange={(v) => update((c) => void (c.server.network.discovery = v), true)} />
        </Row>
      </Section>
      <Section title="Who may connect" description="There is no password by design; these filters limit which addresses can connect.">
        <Row label="Same subnet only" hint="Only computers on the same local network segment.">
          <Switch label="Same subnet only" checked={net.same_subnet_only} onChange={(v) => update((c) => void (c.server.network.same_subnet_only = v), true)} />
        </Row>
        <Row label="Allow only" hint="One address or range per line. Empty = everyone.">
          <ListEditor label="Allow list" values={net.allow_list} onChange={(v) => update((c) => void (c.server.network.allow_list = v), true)} />
        </Row>
        <Row label="Block" hint="Always refused.">
          <ListEditor label="Block list" values={net.block_list} onChange={(v) => update((c) => void (c.server.network.block_list = v), true)} />
        </Row>
      </Section>
    </Page>
  );
}
