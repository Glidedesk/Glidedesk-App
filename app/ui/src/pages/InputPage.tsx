import { useEffect, useRef, useState } from "react";
import type { Config, ModifierKey } from "../lib/types";
import { NumberInput, Page, Row, Section, Select, Switch, TextInput } from "../components/ui";

type Update = (m: (c: Config) => void, now?: boolean) => void;

const HOTKEY = /^((ctrl|control|alt|option|opt|shift|cmd|command|meta|win|super)\s*\+\s*)*[a-z0-9]+$|^f([1-9]|1[0-9]|2[0-4])$|^$/i;

function Hotkey({ label, hint, value, onChange }: { label: string; hint?: string; value: string; onChange: (v: string) => void }) {
  return (
    <Row label={label} hint={hint}>
      <TextInput label={label} value={value} placeholder="off" width="w-44" invalid={!HOTKEY.test(value.trim())} onChange={onChange} />
    </Row>
  );
}

export function InputPage({ config, update }: { config: Config; update: Update }) {
  const h = config.server.hotkeys;
  const sw = config.server.switching;
  const setH = (k: keyof typeof h) => (v: string) => update((c) => void (c.server.hotkeys[k] = v));
  return (
    <Page title="Keyboard & Mouse" subtitle="Hotkeys and the rules for moving between screens.">
      <Section title="Hotkeys" description="Written like Ctrl+Alt+L. Leave empty to turn one off. Cmd means the Windows key on a PC.">
        <Hotkey label="Lock cursor to this screen" hint="For games and full-screen apps." value={h.lock_cursor} onChange={setH("lock_cursor")} />
        <Hotkey label="Back to this computer" value={h.switch_home} onChange={setH("switch_home")} />
        <Hotkey label="Next computer" value={h.switch_next} onChange={setH("switch_next")} />
        <Hotkey label="Previous computer" value={h.switch_previous} onChange={setH("switch_previous")} />
        <Hotkey label="Reconnect all" value={h.reconnect_all} onChange={setH("reconnect_all")} />
        <Hotkey label="Identify screens" value={h.identify} onChange={setH("identify")} />
      </Section>
      <Section title="Switching" description="Stops the cursor from jumping by accident.">
        <Row label="Wait at the edge" hint="Keep pushing for this long before switching (0 = immediately).">
          <NumberInput label="Delay" value={sw.delay_ms} min={0} max={5000} step={50} unit="ms" onChange={(v) => update((c) => void (c.server.switching.delay_ms = v))} />
        </Row>
        <Row label="Double-tap the edge" hint="Hit the edge twice within this time (0 = off).">
          <NumberInput label="Double tap" value={sw.double_tap_ms} min={0} max={2000} step={50} unit="ms" onChange={(v) => update((c) => void (c.server.switching.double_tap_ms = v))} />
        </Row>
        <Row label="Hold a key to switch">
          <Select<"none" | ModifierKey>
            label="Modifier"
            value={sw.modifier ?? "none"}
            onChange={(v) => update((c) => void (c.server.switching.modifier = v === "none" ? null : v), true)}
            options={[
              ["none", "Not needed"],
              ["shift", "Shift"],
              ["ctrl", "Ctrl"],
              ["alt", "Alt / Option"],
              ["meta", "Cmd / Windows"],
            ]}
          />
        </Row>
        <Row label="Dead corners" hint="No switching this close to a screen corner.">
          <NumberInput label="Dead corners" value={sw.dead_corner_px} min={0} max={500} unit="px" onChange={(v) => update((c) => void (c.server.switching.dead_corner_px = v))} />
        </Row>
        <Row label="Wrap around" hint="Leaving the last screen comes back in on the first.">
          <Switch label="Wrap around" checked={sw.wrap} onChange={(v) => update((c) => void (c.server.switching.wrap = v), true)} />
        </Row>
        <Row label="Not while a full-screen app is open" hint="Games and presentations keep the cursor.">
          <Switch label="Block in full screen" checked={sw.block_fullscreen} onChange={(v) => update((c) => void (c.server.switching.block_fullscreen = v), true)} />
        </Row>
        {sw.block_fullscreen && (
          <Row label="Apps that may still switch" hint="Program names, one per line (e.g. code.exe, Safari).">
            <AllowList values={config.server.fullscreen_allow} onChange={(v) => update((c) => void (c.server.fullscreen_allow = v), true)} />
          </Row>
        )}
      </Section>
    </Page>
  );
}

/** Controlled list editor that follows external reloads unless it is being edited. */
function AllowList({ values, onChange }: { values: string[]; onChange: (v: string[]) => void }) {
  const [text, setText] = useState(values.join("\n"));
  const editing = useRef(false);
  const joined = values.join("\n");
  useEffect(() => {
    if (!editing.current) setText(joined);
  }, [joined]);
  return (
    <textarea
      aria-label="Allowed full-screen apps"
      className="h-20 w-60 rounded-lg border border-line bg-panel px-2 py-1 font-mono text-[12.5px]"
      value={text}
      spellCheck={false}
      onFocus={() => (editing.current = true)}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => {
        editing.current = false;
        onChange(text.split("\n").map((l) => l.trim().toLowerCase()).filter(Boolean));
      }}
    />
  );
}
