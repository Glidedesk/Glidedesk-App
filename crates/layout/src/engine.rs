//! The switching engine: decides where the cursor lives.
//!
//! Pure and synchronous — the server core feeds it motion and gets back an
//! [`Outcome`]. All edge logic runs on the server, which tracks a virtual
//! cursor inside the active client's desktop (the client only warps).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use glidedesk_proto::{DeviceId, MonitorInfo, Point, Rect, Side};

use crate::edges::{EdgeRun, select};
use crate::guard::{Decision, EdgeContext, GuardState, SwitchPolicy};
use crate::model::{LinkSpec, Mapping, MonitorSelection, Span};

/// One machine and its monitors in native desktop coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub id: DeviceId,
    pub monitors: Vec<MonitorInfo>,
}

/// Problems found while building a layout; shown in the UI and tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    UnknownMachine(DeviceId),
    /// Chosen monitors are not connected; the link uses all monitors.
    SelectionFellBack {
        machine: DeviceId,
        side: Side,
    },
    /// The side has no outer edge on the selected monitors.
    NoOuterEdge {
        machine: DeviceId,
        side: Side,
    },
    InvalidSpan {
        from: DeviceId,
        to: DeviceId,
    },
}

#[derive(Clone, Debug)]
struct ResolvedLink {
    from: DeviceId,
    to: DeviceId,
    side: Side,
    from_mapping: Mapping,
    /// `PerMonitor` only for the derived return path of a per-monitor link:
    /// land on the monitor the cursor originally left from.
    to_mapping: Mapping,
    from_span: Span,
    to_span: Span,
    from_run: EdgeRun,
    to_run: EdgeRun,
}

/// Immutable, validated geometry of all machines and links.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    machines: HashMap<DeviceId, Vec<MonitorInfo>>,
    links: Vec<ResolvedLink>,
    warnings: Vec<Warning>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Exit {
    link: usize,
    seg: usize,
    /// Position inside the link's from-span, 0..1.
    u: f64,
}

impl Layout {
    #[must_use]
    pub fn build(machines: impl IntoIterator<Item = Machine>, specs: &[LinkSpec]) -> Self {
        let machines: HashMap<_, _> = machines.into_iter().map(|m| (m.id, m.monitors)).collect();
        let mut out = Layout { machines, ..Default::default() };

        for spec in specs {
            if let Some(link) = out.resolve(spec, Mapping::Continuous) {
                out.links.push(link);
            }
        }
        // Derived return paths, unless the user defined one explicitly.
        for spec in specs {
            let explicit =
                specs.iter().any(|o| o.from == spec.to && o.to == spec.from && o.side == spec.side.opposite());
            if explicit {
                continue;
            }
            let reverse = LinkSpec {
                from: spec.to,
                to: spec.from,
                side: spec.side.opposite(),
                handover: spec.entry.clone(),
                mapping: Mapping::Continuous,
                from_span: spec.to_span,
                entry: spec.handover.clone(),
                to_span: spec.from_span,
            };
            if let Some(link) = out.resolve(&reverse, spec.mapping) {
                out.links.push(link);
            }
        }
        out.warnings.dedup();
        out
    }

    fn run(&mut self, machine: DeviceId, side: Side, sel: &MonitorSelection) -> Option<EdgeRun> {
        let Some(monitors) = self.machines.get(&machine) else {
            self.warnings.push(Warning::UnknownMachine(machine));
            return None;
        };
        let chosen = select(monitors, sel);
        if chosen.fell_back {
            self.warnings.push(Warning::SelectionFellBack { machine, side });
        }
        let run = EdgeRun::build(monitors, side, &chosen.mask);
        if run.is_empty() {
            self.warnings.push(Warning::NoOuterEdge { machine, side });
            return None;
        }
        Some(run)
    }

    fn resolve(&mut self, spec: &LinkSpec, to_mapping: Mapping) -> Option<ResolvedLink> {
        if !spec.from_span.is_valid() || !spec.to_span.is_valid() || spec.from == spec.to {
            self.warnings.push(Warning::InvalidSpan { from: spec.from, to: spec.to });
            return None;
        }
        let from_run = self.run(spec.from, spec.side, &spec.handover)?;
        let to_run = self.run(spec.to, spec.side.opposite(), &spec.entry)?;
        Some(ResolvedLink {
            from: spec.from,
            to: spec.to,
            side: spec.side,
            from_mapping: spec.mapping,
            to_mapping,
            from_span: spec.from_span,
            to_span: spec.to_span,
            from_run,
            to_run,
        })
    }

    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    #[must_use]
    pub fn monitors(&self, machine: &DeviceId) -> Option<&[MonitorInfo]> {
        self.machines.get(machine).map(Vec::as_slice)
    }

    fn exit(&self, machine: DeviceId, side: Side, p: Point, skip: &dyn Fn(DeviceId) -> bool) -> Option<Exit> {
        self.links.iter().enumerate().find_map(|(i, l)| {
            if l.from != machine || l.side != side || skip(l.to) {
                return None;
            }
            let seg = l.from_run.locate(p)?;
            let t = match l.from_mapping {
                Mapping::Continuous => l.from_run.t_continuous(seg, p),
                Mapping::PerMonitor => l.from_run.t_segment(seg, p),
            };
            l.from_span.contains(t).then(|| Exit { link: i, seg, u: l.from_span.to_local(t) })
        })
    }

    fn entry(&self, exit: Exit, return_seg: Option<usize>) -> Option<Point> {
        let l = &self.links[exit.link];
        let v = l.to_span.to_global(exit.u);
        match l.to_mapping {
            Mapping::Continuous => l.to_run.point_continuous(v).map(|(_, p)| p),
            Mapping::PerMonitor => {
                let seg = return_seg.filter(|s| *s < l.to_run.segments.len()).unwrap_or(0);
                l.to_run.point_in_segment(seg, v)
            }
        }
    }

    /// Machines reachable by walking links from `start` in direction `side`.
    fn walk(&self, start: DeviceId, side: Side) -> DeviceId {
        let mut cur = start;
        let mut seen = HashSet::from([start]);
        while let Some(next) = self.links.iter().find(|l| l.from == cur && l.side == side).map(|l| l.to) {
            if !seen.insert(next) {
                break;
            }
            cur = next;
        }
        cur
    }
}

/// Where the cursor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Local,
    Remote { machine: DeviceId, pos: Point },
}

/// What the server core must do after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// Hand control to `machine` at `pos` (from local, or from `previous` client).
    Enter {
        machine: DeviceId,
        pos: Point,
        previous: Option<DeviceId>,
    },
    /// Virtual cursor moved on the active client.
    Move {
        pos: Point,
    },
    /// Control returns to this machine; warp the local cursor to `pos`.
    Return {
        pos: Point,
        previous: DeviceId,
    },
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    machine: DeviceId,
    side: Side,
    at: Point,
    ctx: EdgeContext,
    deadline: Instant,
}

#[derive(Debug)]
pub struct Engine {
    layout: Layout,
    local: DeviceId,
    policy: SwitchPolicy,
    focus: Focus,
    guard: GuardState,
    locked: bool,
    unavailable: HashSet<DeviceId>,
    speed: HashMap<DeviceId, f64>,
    remainder: (f64, f64),
    /// (from, to) → segment of `from`'s run the cursor last left through.
    last_exit: HashMap<(DeviceId, DeviceId), usize>,
    local_pos: Point,
    pending: Option<Pending>,
}

impl Engine {
    #[must_use]
    pub fn new(local: DeviceId, layout: Layout, policy: SwitchPolicy) -> Self {
        Self {
            layout,
            local,
            policy,
            focus: Focus::Local,
            guard: GuardState::default(),
            locked: false,
            unavailable: HashSet::new(),
            speed: HashMap::new(),
            remainder: (0.0, 0.0),
            last_exit: HashMap::new(),
            local_pos: Point::default(),
            pending: None,
        }
    }

    #[must_use]
    pub const fn focus(&self) -> Focus {
        self.focus
    }

    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Replace geometry (monitor change, layout edit). Keeps focus if still valid.
    pub fn set_layout(&mut self, layout: Layout) -> Outcome {
        self.layout = layout;
        self.pending = None;
        self.guard.reset();
        if let Focus::Remote { machine, pos } = self.focus {
            match self.layout.monitors(&machine) {
                Some(mons) if !mons.is_empty() => {
                    let clamped = clamp_to_desktop(mons, pos);
                    self.focus = Focus::Remote { machine, pos: clamped };
                    if clamped != pos {
                        return Outcome::Move { pos: clamped };
                    }
                }
                _ => return self.go_home(machine),
            }
        }
        Outcome::None
    }

    pub fn set_policy(&mut self, policy: SwitchPolicy) {
        self.policy = policy;
        self.guard.reset();
        self.pending = None;
    }

    /// Lock the cursor to the current screen (hotkey / tray).
    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
        self.pending = None;
    }

    #[must_use]
    pub const fn locked(&self) -> bool {
        self.locked
    }

    /// Pointer speed multiplier for a client (auto DPI factor × user setting).
    pub fn set_speed(&mut self, machine: DeviceId, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            self.speed.insert(machine, speed);
        }
    }

    /// Mark a client online/offline. Offline clients are walls; if the active
    /// client goes offline, control returns home immediately.
    pub fn set_available(&mut self, machine: DeviceId, available: bool) -> Outcome {
        if available {
            self.unavailable.remove(&machine);
            return Outcome::None;
        }
        self.unavailable.insert(machine);
        match self.focus {
            Focus::Remote { machine: m, .. } if m == machine => self.go_home(machine),
            _ => Outcome::None,
        }
    }

    fn go_home(&mut self, previous: DeviceId) -> Outcome {
        self.focus = Focus::Local;
        self.pending = None;
        self.guard.reset();
        Outcome::Return { pos: self.local_pos, previous }
    }

    /// Jump straight to a machine (hotkey "switch to N").
    pub fn switch_to(&mut self, machine: DeviceId) -> Outcome {
        if machine == self.local {
            return match self.focus {
                Focus::Remote { machine: prev, .. } => self.go_home(prev),
                Focus::Local => Outcome::None,
            };
        }
        if self.unavailable.contains(&machine) {
            return Outcome::None;
        }
        let Some(mons) = self.layout.monitors(&machine) else { return Outcome::None };
        let Some(target) = mons.iter().find(|m| m.primary).or_else(|| mons.first()) else {
            return Outcome::None;
        };
        let pos = center(&target.bounds);
        self.enter(machine, pos)
    }

    fn enter(&mut self, machine: DeviceId, pos: Point) -> Outcome {
        self.pending = None;
        self.guard.reset();
        self.remainder = (0.0, 0.0);
        if machine == self.local {
            return match self.focus {
                Focus::Remote { machine: prev, .. } => {
                    self.focus = Focus::Local;
                    self.local_pos = pos;
                    Outcome::Return { pos, previous: prev }
                }
                Focus::Local => Outcome::None,
            };
        }
        let previous = match self.focus {
            Focus::Remote { machine: m, .. } => Some(m),
            Focus::Local => None,
        };
        self.focus = Focus::Remote { machine, pos };
        Outcome::Enter { machine, pos, previous }
    }

    /// Local cursor moved (server has control). `delta` is the raw push
    /// direction, needed because the OS clamps the position at edges.
    pub fn on_local_move(&mut self, pos: Point, dx: i32, dy: i32, ctx: EdgeContext) -> Outcome {
        self.local_pos = pos;
        if self.focus != Focus::Local {
            return Outcome::None;
        }
        self.try_edges(self.local, pos, dx, dy, ctx).unwrap_or(Outcome::None)
    }

    /// Relative motion while a client has control.
    #[allow(clippy::cast_possible_truncation)]
    pub fn on_remote_move(&mut self, dx: i32, dy: i32, ctx: EdgeContext) -> Outcome {
        let Focus::Remote { machine, pos } = self.focus else { return Outcome::None };
        let Some(mons) = self.layout.monitors(&machine) else { return self.go_home(machine) };
        let speed = self.speed.get(&machine).copied().unwrap_or(1.0);
        let fx = f64::from(dx) * speed + self.remainder.0;
        let fy = f64::from(dy) * speed + self.remainder.1;
        let (ix, iy) = (fx.trunc(), fy.trunc());
        self.remainder = (fx - ix, fy - iy);
        let (ix, iy) = (ix as i32, iy as i32);

        let wanted = Point::new(pos.x.saturating_add(ix), pos.y.saturating_add(iy));
        if mons.iter().any(|m| m.bounds.contains(wanted)) {
            self.guard.away();
            self.pending = None;
            self.focus = Focus::Remote { machine, pos: wanted };
            return Outcome::Move { pos: wanted };
        }
        // Leaving every monitor: clamp to the one we are on, then try its edges.
        let current = mons.iter().find(|m| m.bounds.contains(pos)).map_or_else(|| nearest(mons, pos), |m| m.bounds);
        let clamped = current.clamp(wanted);
        let push_x = wanted.x - clamped.x;
        let push_y = wanted.y - clamped.y;
        if let Some(out) = self.try_edges(machine, clamped, push_x, push_y, ctx) {
            return out;
        }
        if clamped != pos {
            self.focus = Focus::Remote { machine, pos: clamped };
            return Outcome::Move { pos: clamped };
        }
        Outcome::None
    }

    /// Re-check a delayed switch (call at the `WaitUntil` time).
    pub fn poll(&mut self, now: Instant) -> Outcome {
        let Some(p) = self.pending else { return Outcome::None };
        if now < p.deadline {
            return Outcome::None;
        }
        self.pending = None;
        let still_there = match self.focus {
            Focus::Local => p.machine == self.local && self.local_pos == p.at,
            Focus::Remote { machine, pos } => machine == p.machine && pos == p.at,
        };
        if !still_there {
            return Outcome::None;
        }
        let ctx = EdgeContext { now, ..p.ctx };
        let (dx, dy) = match p.side {
            Side::Left => (-1, 0),
            Side::Right => (1, 0),
            Side::Top => (0, -1),
            Side::Bottom => (0, 1),
        };
        self.try_edges(p.machine, p.at, dx, dy, ctx).unwrap_or(Outcome::None)
    }

    /// Next time `poll` should be called, if a delayed switch is pending.
    #[must_use]
    pub fn pending_deadline(&self) -> Option<Instant> {
        self.pending.map(|p| p.deadline)
    }

    fn try_edges(&mut self, machine: DeviceId, at: Point, dx: i32, dy: i32, ctx: EdgeContext) -> Option<Outcome> {
        let mut sides = [None, None];
        if dx > 0 {
            sides[0] = Some(Side::Right);
        } else if dx < 0 {
            sides[0] = Some(Side::Left);
        }
        if dy > 0 {
            sides[1] = Some(Side::Bottom);
        } else if dy < 0 {
            sides[1] = Some(Side::Top);
        }
        let mut at_edge = false;
        for side in sides.into_iter().flatten() {
            match self.try_side(machine, side, at, ctx) {
                SideResult::NotAtEdge => {}
                SideResult::Blocked => at_edge = true,
                SideResult::Switched(o) => return Some(o),
            }
        }
        if !at_edge {
            self.guard.away();
            self.pending = None;
        }
        None
    }

    fn try_side(&mut self, machine: DeviceId, side: Side, at: Point, ctx: EdgeContext) -> SideResult {
        if self.locked {
            return SideResult::NotAtEdge;
        }
        let unavailable = self.unavailable.clone();
        let skip = |id: DeviceId| unavailable.contains(&id);

        let (target, pos) = if let Some(exit) = self.layout.exit(machine, side, at, &skip) {
            let l = &self.layout.links[exit.link];
            let seg = &l.from_run.segments[exit.seg];
            let corner = self.corner_distance(machine, seg.monitor, side, at);
            let return_seg = self.last_exit.get(&(l.to, l.from)).copied();
            let Some(pos) = self.layout.entry(exit, return_seg) else { return SideResult::NotAtEdge };
            let (from, to) = (l.from, l.to);
            if let Some(r) = self.decide(machine, side, at, ctx, corner) {
                return r;
            }
            self.last_exit.insert((from, to), exit.seg);
            (to, pos)
        } else if self.policy.wrap {
            let Some((to, pos)) = self.wrap(machine, side, at, &skip) else { return SideResult::NotAtEdge };
            if let Some(r) = self.decide(machine, side, at, ctx, u32::MAX) {
                return r;
            }
            (to, pos)
        } else {
            return SideResult::NotAtEdge;
        };
        SideResult::Switched(self.enter(target, pos))
    }

    /// Why a push against an edge of `machine` at `at` does not switch (for the
    /// log; changes nothing). `None` = it isn't at an outer edge in that direction.
    #[must_use]
    pub fn explain_edge(&self, machine: DeviceId, at: Point, dx: i32, dy: i32, ctx: &EdgeContext) -> Option<String> {
        let sides = [
            (dx > 0).then_some(Side::Right),
            (dx < 0).then_some(Side::Left),
            (dy > 0).then_some(Side::Bottom),
            (dy < 0).then_some(Side::Top),
        ];
        let mons = self.layout.monitors(&machine)?;
        for side in sides.into_iter().flatten() {
            let r = mons.iter().find(|m| m.bounds.contains(at)).map(|m| m.bounds)?;
            let on_edge = match side {
                Side::Left => at.x == r.left(),
                Side::Right => at.x == r.right() - 1,
                Side::Top => at.y == r.top(),
                Side::Bottom => at.y == r.bottom() - 1,
            };
            if !on_edge {
                continue;
            }
            if self.locked {
                return Some("the cursor is locked to this screen".into());
            }
            let skip = |id: DeviceId| self.unavailable.contains(&id);
            let Some(exit) = self.layout.exit(machine, side, at, &skip) else {
                let linked: Vec<String> = self
                    .layout
                    .links
                    .iter()
                    .filter(|l| l.from == machine)
                    .map(|l| format!("{:?}{}", l.side, if skip(l.to) { " (offline)" } else { "" }).to_lowercase())
                    .collect();
                let side = format!("{side:?}").to_lowercase();
                return Some(if linked.is_empty() {
                    format!("no computer is placed next to this screen yet (pushed the {side} edge) — see Layout")
                } else {
                    format!(
                        "no computer on the {side} edge — the other computer is on the {} (Layout)",
                        linked.join(", ")
                    )
                });
            };
            let l = &self.layout.links[exit.link];
            let corner = self.corner_distance(machine, l.from_run.segments[exit.seg].monitor, side, at);
            let p = &self.policy;
            return Some(if let Some(m) = p.modifier.filter(|m| !ctx.mods.has(*m)) {
                format!("hold {m:?} to switch (Keyboard & Mouse → switching)")
            } else if p.block_fullscreen && ctx.fullscreen {
                "a full-screen app is in front (Keyboard & Mouse → switching)".into()
            } else if corner < p.dead_corner_px {
                "too close to a screen corner".into()
            } else if p.double_tap_ms > 0 {
                "double-tap the edge to switch".into()
            } else if p.delay_ms > 0 {
                "keep pushing: switch delay".into()
            } else {
                "not blocked".into()
            });
        }
        None
    }

    /// `None` = allowed; `Some` = stop here with this result.
    fn decide(
        &mut self,
        machine: DeviceId,
        side: Side,
        at: Point,
        ctx: EdgeContext,
        corner: u32,
    ) -> Option<SideResult> {
        match self.guard.evaluate((machine, side), &ctx, &self.policy, corner) {
            Decision::Allow => None,
            Decision::Deny => Some(SideResult::Blocked),
            Decision::WaitUntil(deadline) => {
                self.pending = Some(Pending { machine, side, at, ctx, deadline });
                Some(SideResult::Blocked)
            }
        }
    }

    fn corner_distance(&self, machine: DeviceId, monitor: usize, side: Side, at: Point) -> u32 {
        let Some(r) = self.layout.monitors(&machine).and_then(|m| m.get(monitor)).map(|m| m.bounds) else {
            return u32::MAX;
        };
        let (a, s, e) =
            if side.is_vertical_edge() { (at.y, r.top(), r.bottom() - 1) } else { (at.x, r.left(), r.right() - 1) };
        u32::try_from((a - s).min(e - a).max(0)).unwrap_or(0)
    }

    fn wrap(
        &self,
        machine: DeviceId,
        side: Side,
        at: Point,
        skip: &dyn Fn(DeviceId) -> bool,
    ) -> Option<(DeviceId, Point)> {
        let far = self.layout.walk(machine, side.opposite());
        if far == machine || skip(far) {
            return None;
        }
        let from = self.layout.monitors(&machine)?;
        let to = self.layout.monitors(&far)?;
        let src = EdgeRun::build(from, side, &vec![true; from.len()]);
        let dst = EdgeRun::build(to, side.opposite(), &vec![true; to.len()]);
        let idx = src.locate(at)?;
        let t = src.t_continuous(idx, at);
        dst.point_continuous(t).map(|(_, p)| (far, p))
    }
}

enum SideResult {
    NotAtEdge,
    Blocked,
    Switched(Outcome),
}

fn center(r: &Rect) -> Point {
    Point::new(r.x + r.w / 2, r.y + r.h / 2)
}

fn nearest(mons: &[MonitorInfo], p: Point) -> Rect {
    mons.iter()
        .map(|m| m.bounds)
        .min_by_key(|r| {
            let c = r.clamp(p);
            i64::from(c.x - p.x).pow(2) + i64::from(c.y - p.y).pow(2)
        })
        .unwrap_or_default()
}

fn clamp_to_desktop(mons: &[MonitorInfo], p: Point) -> Point {
    if mons.iter().any(|m| m.bounds.contains(p)) {
        return p;
    }
    nearest(mons, p).clamp(p)
}
