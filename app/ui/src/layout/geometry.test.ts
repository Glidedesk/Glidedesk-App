import { describe, expect, it } from "vitest";
import { attach, descendants, placeAll, simpleLink, snapTarget, toBox, viewBox } from "./geometry";
import type { MonitorInfo } from "../lib/types";

const mon = (x: number, y: number, w: number, h: number, scale = 1): MonitorInfo => ({
  id: `${x},${y}`,
  name: "m",
  bounds: { x, y, w, h },
  scale,
  primary: x === 0 && y === 0,
});

describe("geometry", () => {
  const S = "s";
  const A = "a";
  const B = "b";
  const boxes = new Map([
    [S, toBox(S, "server", [mon(0, 0, 1000, 500)], "macos")],
    [A, toBox(A, "a", [mon(0, 0, 3000, 1500, 1.5)], "windows")],
    [B, toBox(B, "b", [mon(0, 0, 800, 600)], "macos")],
  ]);

  it("normalises Windows pixels to DIP", () => {
    expect(boxes.get(A)?.w).toBe(2000);
    expect(boxes.get(A)?.h).toBe(1000);
  });

  it("places through links in both directions", () => {
    const p = placeAll(S, boxes, [simpleLink(S, "right", A), simpleLink(B, "right", S)]);
    expect(p.pos.get(A)?.x).toBeGreaterThan(1000);
    expect(p.pos.get(B)?.x).toBeLessThan(0);
    expect(p.unplaced).toEqual([]);
    expect(p.parent.get(A)?.[0]).toBe(S);
  });

  it("reports unplaced machines", () => {
    const p = placeAll(S, boxes, [simpleLink(S, "right", A)]);
    expect(p.unplaced).toEqual([B]);
  });

  it("snaps to the nearest side and never onto its own chain", () => {
    const links = [simpleLink(S, "right", A), simpleLink(A, "right", B)];
    const p = placeAll(S, boxes, links);
    expect(descendants(A, p.parent)).toEqual(new Set([B]));
    const below = snapTarget({ x: 500, y: 900 }, B, boxes, p);
    expect(below).toEqual({ target: S, side: "bottom" });
    const nearB = p.pos.get(B);
    const t = snapTarget({ x: (nearB?.x ?? 0) + 10, y: (nearB?.y ?? 0) + 10 }, A, boxes, p);
    expect(t?.target).not.toBe(B);
  });

  it("attach replaces only the parent link", () => {
    const links = [simpleLink(S, "right", A), simpleLink(A, "right", B)];
    const p = placeAll(S, boxes, links);
    const next = attach(links, p, A, S, "left");
    expect(next).toContainEqual(simpleLink(A, "right", B));
    expect(next).toContainEqual(simpleLink(S, "left", A));
    expect(next).not.toContainEqual(simpleLink(S, "right", A));
  });

  it("view box contains everything", () => {
    const p = placeAll(S, boxes, [simpleLink(S, "right", A)]);
    const vb = viewBox(boxes, p);
    expect(vb.x).toBeLessThan(0);
    expect(vb.x + vb.w).toBeGreaterThan(1000 + 60 + 2000);
  });
});
