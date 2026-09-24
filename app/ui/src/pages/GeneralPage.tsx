import type { AgentStatus, Config, Role } from "../lib/types";
import { NumberInput, Page, Row, Section, Select, Switch, TextInput } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;

export function GeneralPage({ status, config, update }: { status: AgentStatus; config: Config; update: Update }) {
  const g = config.general;
  const isServer = status.role === "server";
  const h = config.server.health;
  const v = config.server.visuals;
  return (
    <Page title="General" subtitle="This computer, startup and notifications.">
      <Section title="This computer">
        <Row label="Name" hint="Shown on other computers. Empty uses the computer name.">
          <TextInput label="Name" value={config.device.name} placeholder={status.device_name} onChange={(val) => update((c) => void (c.device.name = val.slice(0, 64)))} />
        </Row>
        <Row label="Role" hint="Server shares its keyboard and mouse; Client is controlled by a server.">
          <Select<Role>
            label="Role"
            value={config.device.role}
            onChange={(r) => update((c) => void (c.device.role = r), true)}
            options={[
              ["server", "Server — shares its keyboard and mouse"],
              ["client", "Client — controlled by a server"],
            ]}
          />
        </Row>
      </Section>
      <Section title="Startup & appearance">
        <Row label="Start at login">
          <Switch label="Start at login" checked={g.start_at_login} onChange={(val) => update((c) => void (c.general.start_at_login = val), true)} />
        </Row>
        <Row label="Notifications">
          <Switch label="Notifications" checked={g.notifications} onChange={(val) => update((c) => void (c.general.notifications = val), true)} />
        </Row>
        <Row label="Ask before quitting">
          <Switch label="Confirm quit" checked={g.confirm_quit} onChange={(val) => update((c) => void (c.general.confirm_quit = val), true)} />
        </Row>
        <Row label="Appearance">
          <Select
            label="Theme"
            value={g.theme}
            onChange={(t) => update((c) => void (c.general.theme = t), true)}
            options={[
              ["system", "Match system"],
              ["light", "Light"],
              ["dark", "Dark"],
            ]}
          />
        </Row>
      </Section>
      {isServer && (
        <>
          <Section title="Health checks" description="How the server notices a computer went offline.">
            <Row label="Check every">
              <NumberInput label="Interval" value={h.interval_ms} min={500} max={10000} step={100} unit="ms" onChange={(val) => update((c) => void (c.server.health.interval_ms = val))} />
            </Row>
            <Row label="Offline after" hint={`≈ ${((h.interval_ms * h.miss_threshold) / 1000).toFixed(1)} s without an answer.`}>
              <NumberInput label="Missed checks" value={h.miss_threshold} min={1} max={20} unit="missed checks" onChange={(val) => update((c) => void (c.server.health.miss_threshold = val))} />
            </Row>
            <Row label="Slow above">
              <NumberInput label="Slow threshold" value={h.degraded_latency_ms} min={5} max={5000} unit="ms" onChange={(val) => update((c) => void (c.server.health.degraded_latency_ms = val))} />
            </Row>
          </Section>
          <Section title="Messages">
            <Row label="When a computer connects">
              <Switch label="Notify connect" checked={v.notify_connect} onChange={(val) => update((c) => void (c.server.visuals.notify_connect = val), true)} />
            </Row>
            <Row label="When a computer goes offline">
              <Switch label="Notify offline" checked={v.notify_offline} onChange={(val) => update((c) => void (c.server.visuals.notify_offline = val), true)} />
            </Row>
            <Row label="Identify screens shows for">
              <NumberInput label="Identify seconds" value={v.identify_seconds} min={1} max={30} unit="s" onChange={(val) => update((c) => void (c.server.visuals.identify_seconds = val))} />
            </Row>
          </Section>
        </>
      )}
    </Page>
  );
}
