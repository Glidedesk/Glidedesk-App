// Drag computers around the server to decide which edge leads where
// (PLAN §6.1). Keyboard: Tab to a computer, arrow keys move it to that side.
import { useMemo, useRef, useState, type PointerEvent as RPointerEvent, type KeyboardEvent } from "react";
import type { AgentStatus, Config, DeviceId, LinkSpec, Side } from "../lib/types";
import { healthLabel, healthTone } from "../lib/format";
import { attach, placeAll, snapTarget, toBox, viewBox, type MachineBox } from "./geometry";

interface Props {
  status: AgentStatus;
  config: Config;
  selected: DeviceId | null;
  onSelect: (id: DeviceId | null) => void;
  onLinks: (links: LinkSpec[]) => void;
}

const toneFill = { ok: "var(--ok)", warn: "var(--warn)", muted: "var(--muted)", busy: "var(--accent)" } as const;

export function useBoxes(status: AgentStatus, config: Config) {
  return useMemo(() => {
    const boxes = new Map<DeviceId, MachineBox>();
    const server = status.server;
    const localId = config.device.id;
    boxes.set(localId, toBox(localId, status.device_name || "This computer", server?.local.monitors ?? [], status.platform));
    for (const c of config.server.clients) {
      const view = server?.clients.find((v) => v.id === c.id);
      boxes.set(c.id, toBox(c.id, view?.name || c.name || "Computer", view?.monitors ?? [], view?.platform ?? null));
    }
    return boxes;
  }, [status, config]);
}

export function LayoutEditor({ status, config, selected, onSelect, onLinks }: Props) {
  const boxes = useBoxes(status, config);
  const localId = config.device.id;
  const links = config.layout.link;
  const placement = useMemo(() => placeAll(localId, boxes, links), [localId, boxes, links]);
  const vb = useMemo(() => viewBox(boxes, placement), [boxes, placement]);
  const svg = useRef<SVGSVGElement>(null);
  const [drag, setDrag] = useState<{ id: DeviceId; x: number; y: number; moved?: boolean } | null>(null);

  const clientState = (id: DeviceId) => status.server?.clients.find((c) => c.id === id)?.state ?? "never-connected";

  const toSvg = (e: { clientX: number; clientY: number }) => {
    const el = svg.current;
    if (!el) return { x: 0, y: 0 };
    const pt = el.createSVGPoint();
    pt.x = e.clientX;
    pt.y = e.clientY;
    const m = el.getScreenCTM();
    const p = m ? pt.matrixTransform(m.inverse()) : pt;
    return { x: p.x, y: p.y };
  };

  const snap = drag ? snapTarget({ x: drag.x, y: drag.y }, drag.id, boxes, placement) : null;

  const startDrag = (id: DeviceId) => (e: RPointerEvent) => {
    if (id === localId) return;
    (e.target as Element).setPointerCapture?.(e.pointerId);
    onSelect(id);
    setDrag({ id, ...toSvg(e) });
  };
  const moveDrag = (e: RPointerEvent) => {
    if (drag) setDrag({ ...drag, ...toSvg(e), moved: true });
  };
  const endDrag = () => {
    // A plain click only selects; only a real drag moves the computer.
    if (drag?.moved && snap) onLinks(attach(links, placement, drag.id, snap.target, snap.side));
    setDrag(null);
  };

  const onKey = (id: DeviceId) => (e: KeyboardEvent) => {
    const map: Record<string, Side> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "top", ArrowDown: "bottom" };
    const side = map[e.key];
    if (e.key === "Enter" || e.key === " ") {
      onSelect(id);
      e.preventDefault();
      return;
    }
    if (!side || id === localId) return;
    e.preventDefault();
    const parent = placement.parent.get(id)?.[0] ?? localId;
    onLinks(attach(links, placement, id, parent, side));
  };

  const tile = (id: DeviceId, x: number, y: number, ghost = false) => {
    const b = boxes.get(id);
    if (!b) return null;
    const isLocal = id === localId;
    const state = clientState(id);
    const tone = isLocal ? "ok" : healthTone(state);
    const isFocus = status.server?.focus === id || (isLocal && status.server?.focus === null);
    const sel = selected === id;
    const fontSize = Math.max(90, Math.min(b.w, b.h) * 0.12);
    return (
      <g
        key={`${id}${ghost ? "-ghost" : ""}`}
        transform={`translate(${x} ${y})`}
        opacity={ghost ? 0.55 : !isLocal && (state === "offline" || state === "never-connected") ? 0.6 : 1}
        className={isLocal || ghost ? "" : "cursor-grab"}
        role={ghost ? undefined : "button"}
        tabIndex={ghost ? -1 : 0}
        aria-label={`${b.name}${isLocal ? " (this computer)" : `, ${healthLabel(state)}`}`}
        aria-pressed={sel}
        onPointerDown={ghost ? undefined : startDrag(id)}
        onClick={() => onSelect(id)}
        onKeyDown={ghost ? undefined : onKey(id)}
      >
        <rect width={b.w} height={b.h} rx={60} fill="var(--panel)" stroke={sel ? "var(--accent)" : "var(--border)"} strokeWidth={sel ? 36 : 16} />
        {b.monitors.map((m) => (
          <rect
            key={m.id}
            x={m.rect.x + 40}
            y={m.rect.y + 40}
            width={Math.max(10, m.rect.w - 80)}
            height={Math.max(10, m.rect.h - 80)}
            rx={24}
            fill={isLocal ? "var(--accent-soft)" : "var(--panel-2)"}
            stroke={m.primary ? "var(--accent)" : "var(--border)"}
            strokeWidth={10}
          />
        ))}
        <text x={b.w / 2} y={b.h / 2} textAnchor="middle" dominantBaseline="middle" fontSize={fontSize} fontWeight={600} fill="var(--text)">
          {b.name}
        </text>
        <text x={b.w / 2} y={b.h / 2 + fontSize * 1.2} textAnchor="middle" fontSize={fontSize * 0.7} fill="var(--muted)">
          {isLocal ? "this computer" : healthLabel(state)}
        </text>
        <circle cx={b.w - 110} cy={110} r={45} fill={toneFill[tone]} />
        {isFocus && <circle cx={110} cy={110} r={45} fill="var(--accent)" />}
      </g>
    );
  };

  const preview = (() => {
    if (!drag || !snap) return null;
    const t = placement.pos.get(snap.target);
    const tb = boxes.get(snap.target);
    const db = boxes.get(drag.id);
    if (!t || !tb || !db) return null;
    const w = 50;
    const r = {
      right: { x: t.x + tb.w + 10, y: t.y, w, h: tb.h },
      left: { x: t.x - w - 10, y: t.y, w, h: tb.h },
      top: { x: t.x, y: t.y - w - 10, w: tb.w, h: w },
      bottom: { x: t.x, y: t.y + tb.h + 10, w: tb.w, h: w },
    }[snap.side];
    return <rect {...{ x: r.x, y: r.y, width: r.w, height: r.h }} rx={20} fill="var(--accent)" opacity={0.8} />;
  })();

  return (
    <div className="relative h-full min-h-[320px] w-full overflow-hidden rounded-2xl border border-line bg-canvas">
      <svg
        ref={svg}
        className="h-full w-full touch-none select-none"
        viewBox={`${vb.x} ${vb.y} ${vb.w} ${vb.h}`}
        onPointerMove={moveDrag}
        onPointerUp={endDrag}
        onPointerCancel={() => setDrag(null)}
        onClick={(e) => {
          if (e.target === svg.current) onSelect(null);
        }}
      >
        {[...placement.pos].map(([id, p]) => (drag?.id === id ? null : tile(id, p.x, p.y)))}
        {preview}
        {drag &&
          (() => {
            const b = boxes.get(drag.id);
            return b ? tile(drag.id, drag.x - b.w / 2, drag.y - b.h / 2, true) : null;
          })()}
      </svg>
      {placement.unplaced.length > 0 && (
        <div className="absolute inset-x-3 bottom-3 flex flex-wrap items-center gap-2 rounded-xl border border-line bg-panel/95 px-3 py-2 shadow-sm">
          <span className="text-muted">Waiting to be placed — drag onto a side:</span>
          {placement.unplaced.map((id) => {
            const b = boxes.get(id);
            return (
              <button
                key={id}
                type="button"
                className="cursor-grab rounded-lg border border-line bg-panel-2 px-2.5 py-1 font-medium"
                onPointerDown={(e) => {
                  // Capture so the drag always ends, even if released outside the canvas.
                  e.currentTarget.setPointerCapture(e.pointerId);
                  onSelect(id);
                  setDrag({ id, ...toSvg(e) });
                }}
                onPointerMove={moveDrag}
                onPointerUp={endDrag}
                onPointerCancel={() => setDrag(null)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    onLinks(attach(links, placement, id, localId, "right"));
                  }
                }}
                title="Drag onto the layout, or press Enter to put it on the right"
              >
                {b?.name ?? id.slice(0, 6)}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
