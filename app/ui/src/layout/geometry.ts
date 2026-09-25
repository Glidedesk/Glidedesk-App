// Pure layout maths for the editor: machine boxes, placement from links,
// drop-to-side snapping. Units are device-independent pixels (DIP), so a
// 4K Windows screen at 150% and a Retina Mac look the right size together.
import type { DeviceId, LinkSpec, MonitorInfo, Platform, Rect, Side } from "../lib/types";

export const GAP = 60;
const FALLBACK: MonitorInfo = { id: "unknown", name: "", bounds: { x: 0, y: 0, w: 1920, h: 1080 }, scale: 1, primary: true };

export interface MachineBox {
  id: DeviceId;
  name: string;
  w: number;
  h: number;
  /** Monitors relative to the box origin, in DIP. */
  monitors: { id: string; name: string; rect: Rect; primary: boolean }[];
}

export function toBox(id: DeviceId, name: string, monitors: MonitorInfo[], platform: Platform | null): MachineBox {
  const list = monitors.length > 0 ? monitors : [FALLBACK];
  const k = (m: MonitorInfo) => (platform === "windows" && m.scale > 0 ? 1 / m.scale : 1);
  const rects = list.map((m) => ({
    id: m.id,
    name: m.name,
    primary: m.primary,
    rect: { x: m.bounds.x * k(m), y: m.bounds.y * k(m), w: m.bounds.w * k(m), h: m.bounds.h * k(m) },
  }));
  const minX = Math.min(...rects.map((r) => r.rect.x));
  const minY = Math.min(...rects.map((r) => r.rect.y));
  const maxX = Math.max(...rects.map((r) => r.rect.x + r.rect.w));
  const maxY = Math.max(...rects.map((r) => r.rect.y + r.rect.h));
  return {
    id,
    name,
    w: maxX - minX,
    h: maxY - minY,
    monitors: rects.map((r) => ({ ...r, rect: { ...r.rect, x: r.rect.x - minX, y: r.rect.y - minY } })),
  };
}

export const opposite = (s: Side): Side => ({ left: "right", right: "left", top: "bottom", bottom: "top" })[s] as Side;

export interface Placement {
  pos: Map<DeviceId, { x: number; y: number }>;
  /** machine → [parent, link index] it was placed through */
  parent: Map<DeviceId, [DeviceId, number]>;
  unplaced: DeviceId[];
}

function place(a: { x: number; y: number }, ab: MachineBox, bb: MachineBox, side: Side, span: { start: number; end: number }) {
  const centre = (span.start + span.end) / 2;
  switch (side) {
    case "right":
      return { x: a.x + ab.w + GAP, y: a.y + ab.h * centre - bb.h / 2 };
    case "left":
      return { x: a.x - bb.w - GAP, y: a.y + ab.h * centre - bb.h / 2 };
    case "bottom":
      return { x: a.x + ab.w * centre - bb.w / 2, y: a.y + ab.h + GAP };
    case "top":
      return { x: a.x + ab.w * centre - bb.w / 2, y: a.y - bb.h - GAP };
  }
}

/** Places every machine reachable from `root` through links (either direction). */
export function placeAll(root: DeviceId, boxes: Map<DeviceId, MachineBox>, links: LinkSpec[]): Placement {
  const pos = new Map<DeviceId, { x: number; y: number }>([[root, { x: 0, y: 0 }]]);
  const parent = new Map<DeviceId, [DeviceId, number]>();
  const queue: DeviceId[] = [root];
  while (queue.length > 0) {
    const cur = queue.shift() as DeviceId;
    const curPos = pos.get(cur);
    const curBox = boxes.get(cur);
    if (!curPos || !curBox) continue;
    links.forEach((l, i) => {
      let other: DeviceId | null = null;
      let side: Side = l.side;
      let span = l.from_span;
      if (l.from === cur) other = l.to;
      else if (l.to === cur) {
        other = l.from;
        side = opposite(l.side);
        span = l.to_span;
      }
      if (!other || pos.has(other)) return;
      const ob = boxes.get(other);
      if (!ob) return;
      pos.set(other, place(curPos, curBox, ob, side, span));
      parent.set(other, [cur, i]);
      queue.push(other);
    });
  }
  const unplaced = [...boxes.keys()].filter((id) => !pos.has(id));
  return { pos, parent, unplaced };
}

/** Machines placed through `id` (dragging `id` must not attach it to one of them). */
export function descendants(id: DeviceId, parent: Placement["parent"]): Set<DeviceId> {
  const out = new Set<DeviceId>();
  let grew = true;
  while (grew) {
    grew = false;
    for (const [child, [p]] of parent) {
      if ((p === id || out.has(p)) && !out.has(child)) {
        out.add(child);
        grew = true;
      }
    }
  }
  return out;
}

/** Nearest placed machine and the side of it the point is on. */
export function snapTarget(
  point: { x: number; y: number },
  dragged: DeviceId,
  boxes: Map<DeviceId, MachineBox>,
  placement: Placement,
): { target: DeviceId; side: Side } | null {
  const banned = descendants(dragged, placement.parent);
  banned.add(dragged);
  let best: { target: DeviceId; side: Side; d: number } | null = null;
  for (const [id, p] of placement.pos) {
    if (banned.has(id)) continue;
    const b = boxes.get(id);
    if (!b) continue;
    const cx = p.x + b.w / 2;
    const cy = p.y + b.h / 2;
    const nx = (point.x - cx) / (b.w / 2);
    const ny = (point.y - cy) / (b.h / 2);
    const side: Side = Math.abs(nx) >= Math.abs(ny) ? (nx >= 0 ? "right" : "left") : ny >= 0 ? "bottom" : "top";
    const d = Math.hypot(point.x - cx, point.y - cy);
    if (!best || d < best.d) best = { target: id, side, d };
  }
  return best ? { target: best.target, side: best.side } : null;
}

export function simpleLink(from: DeviceId, side: Side, to: DeviceId): LinkSpec {
  return {
    from,
    to,
    side,
    handover: { mode: "all" },
    // Each screen's edge maps to the whole other edge; the cursor returns to the
    // screen it left from — smooth when screens differ in size.
    mapping: "per-monitor",
    from_span: { start: 0, end: 1 },
    entry: { mode: "all" },
    to_span: { start: 0, end: 1 },
  };
}

/** Attach `client` to `target` on `side`, replacing only its link to its current parent. */
export function attach(links: LinkSpec[], placement: Placement, client: DeviceId, target: DeviceId, side: Side): LinkSpec[] {
  const via = placement.parent.get(client);
  const kept = links.filter((_, i) => !(via && i === via[1]));
  // Also drop any direct link between client and target (it is being replaced).
  const cleaned = kept.filter((l) => !((l.from === client && l.to === target) || (l.from === target && l.to === client)));
  return [...cleaned, simpleLink(target, side, client)];
}

/** Remove a machine from the layout (keeps its entry in the computer list). */
export function detach(links: LinkSpec[], client: DeviceId): LinkSpec[] {
  return links.filter((l) => l.from !== client && l.to !== client);
}

/** Bounding box of placed machines, with margin. */
export function viewBox(boxes: Map<DeviceId, MachineBox>, placement: Placement, margin = 200): Rect {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const [id, p] of placement.pos) {
    const b = boxes.get(id);
    if (!b) continue;
    minX = Math.min(minX, p.x);
    minY = Math.min(minY, p.y);
    maxX = Math.max(maxX, p.x + b.w);
    maxY = Math.max(maxY, p.y + b.h);
  }
  if (!Number.isFinite(minX)) return { x: -1000, y: -600, w: 2000, h: 1200 };
  const w = maxX - minX + margin * 2;
  const h = maxY - minY + margin * 2;
  // Keep a wide aspect so a single screen does not fill everything.
  const minW = Math.max(w, h * 1.6, 5200);
  const minH = Math.max(h, minW / 2.2);
  return { x: minX - margin - (minW - w) / 2, y: minY - margin - (minH - h) / 2, w: minW, h: minH };
}
