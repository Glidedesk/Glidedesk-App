// Settings of the link that places the selected computer:
// side, which monitors hand over, how positions map, and edge offsets.
import type { Config, DeviceId, HandoverMode, LinkSpec, Mapping, MonitorSelection, Side } from "../lib/types";
import { Button, Select, Slider } from "../components/ui";
import type { MachineBox } from "./geometry";
import { detach, type Placement } from "./geometry";

interface Props {
  id: DeviceId;
  config: Config;
  boxes: Map<DeviceId, MachineBox>;
  placement: Placement;
  onLinks: (links: LinkSpec[]) => void;
}

function MonitorPicker({ box, sel, onChange, side }: { box: MachineBox; sel: MonitorSelection; onChange: (s: MonitorSelection) => void; side: Side }) {
  const multi = box.monitors.length > 1;
  const chosen = new Set(sel.monitors ?? []);
  return (
    <div className="space-y-2">
      <Select<HandoverMode>
        label={`Monitors of ${box.name}`}
        value={multi ? sel.mode : "all"}
        onChange={(mode) => onChange({ mode, monitors: mode === "all" ? [] : sel.monitors?.length ? sel.monitors : box.monitors.slice(0, 1).map((m) => m.id) })}
        options={[
          ["all", `All monitors on the ${side} side`],
          ["single", "One monitor"],
          ["selected", "Selected monitors"],
        ]}
      />
      {multi && sel.mode !== "all" && (
        <div className="grid gap-1 pl-1">
          {box.monitors.map((m, i) => (
            <label key={m.id} className="flex items-center gap-2">
              <input
                type={sel.mode === "single" ? "radio" : "checkbox"}
                name={`${box.id}-${side}`}
                checked={chosen.has(m.id)}
                onChange={(e) => {
                  if (sel.mode === "single") onChange({ mode: "single", monitors: [m.id] });
                  else {
                    const next = new Set(chosen);
                    if (e.target.checked) next.add(m.id);
                    else next.delete(m.id);
                    onChange({ mode: "selected", monitors: [...next] });
                  }
                }}
              />
              <span>
                Monitor {i + 1}
                <span className="text-muted"> — {m.name || "display"}{m.primary ? " (main)" : ""}</span>
              </span>
            </label>
          ))}
        </div>
      )}
      {!multi && <p className="text-[12.5px] text-muted">One monitor — nothing to choose.</p>}
    </div>
  );
}

export function LinkPanel({ id, config, boxes, placement, onLinks }: Props) {
  const via = placement.parent.get(id);
  const links = config.layout.link;
  const box = boxes.get(id);
  if (!box) return null;
  if (!via) {
    return <p className="text-muted">Drag {box.name} onto a side of another computer to place it.</p>;
  }
  const [parentId, index] = via;
  const link = links[index];
  const parent = boxes.get(parentId);
  if (!link || !parent) return null;
  // The link may be stored in either direction; show it from the parent's point of view.
  const forward = link.from === parentId;
  const side: Side = forward ? link.side : ({ left: "right", right: "left", top: "bottom", bottom: "top" } as const)[link.side];
  const parentSel = forward ? link.handover : link.entry;
  const childSel = forward ? link.entry : link.handover;
  const parentSpan = forward ? link.from_span : link.to_span;
  const childSpan = forward ? link.to_span : link.from_span;

  const write = (patch: Partial<{ side: Side; parentSel: MonitorSelection; childSel: MonitorSelection; mapping: Mapping; parentSpan: typeof parentSpan; childSpan: typeof childSpan }>) => {
    const next: LinkSpec = {
      from: parentId,
      to: id,
      side: patch.side ?? side,
      handover: patch.parentSel ?? parentSel,
      entry: patch.childSel ?? childSel,
      mapping: patch.mapping ?? link.mapping,
      from_span: patch.parentSpan ?? parentSpan,
      to_span: patch.childSpan ?? childSpan,
    };
    onLinks(links.map((l, i) => (i === index ? next : l)));
  };

  const pct = (v: number) => `${Math.round(v * 100)}%`;
  const span = (label: string, s: typeof parentSpan, set: (s: typeof parentSpan) => void) => (
    <div className="space-y-1">
      <div className="text-[12.5px] text-muted">{label}</div>
      <div className="flex flex-col gap-1">
        <Slider label={`${label} start`} value={s.start} min={0} max={0.95} step={0.05} format={pct} onChange={(v) => set({ start: Math.min(v, s.end - 0.05), end: s.end })} />
        <Slider label={`${label} end`} value={s.end} min={0.05} max={1} step={0.05} format={pct} onChange={(v) => set({ start: s.start, end: Math.max(v, s.start + 0.05) })} />
      </div>
    </div>
  );

  return (
    <div className="space-y-5">
      <div>
        <div className="mb-1 font-semibold">Position</div>
        <div className="flex items-center gap-2">
          <Select<Side>
            label="Side"
            value={side}
            onChange={(s) => write({ side: s })}
            options={[
              ["right", "Right of"],
              ["left", "Left of"],
              ["top", "Above"],
              ["bottom", "Below"],
            ]}
          />
          <span className="font-medium">{parent.name}</span>
        </div>
      </div>
      <div>
        <div className="mb-1 font-semibold">Leaves from {parent.name}</div>
        <MonitorPicker box={parent} sel={parentSel} side={side} onChange={(s) => write({ parentSel: s })} />
      </div>
      <div>
        <div className="mb-1 font-semibold">Arrives on {box.name}</div>
        <MonitorPicker box={box} sel={childSel} side={({ left: "right", right: "left", top: "bottom", bottom: "top" } as const)[side]} onChange={(s) => write({ childSel: s })} />
      </div>
      <div>
        <div className="mb-1 font-semibold">Position mapping</div>
        <Select<Mapping>
          label="Mapping"
          value={link.mapping}
          onChange={(m) => write({ mapping: m })}
          options={[
            ["continuous", "Continuous — monitors form one long edge"],
            ["per-monitor", "Per monitor — each maps to the whole edge"],
          ]}
        />
      </div>
      <details>
        <summary className="cursor-pointer font-semibold">Edge offsets</summary>
        <div className="mt-2 space-y-3">
          {span(`Part of ${parent.name}'s edge`, parentSpan, (s) => write({ parentSpan: s }))}
          {span(`Part of ${box.name}'s edge`, childSpan, (s) => write({ childSpan: s }))}
        </div>
      </details>
      <Button variant="danger" onClick={() => onLinks(detach(links, id))}>
        Remove from layout
      </Button>
    </div>
  );
}
