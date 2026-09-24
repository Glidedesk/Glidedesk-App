//! Persisted layout description (lives in `config.toml`, see PLAN §6.1).

use glidedesk_proto::{DeviceId, MonitorId, Side};
use serde::{Deserialize, Serialize};

/// Which monitors of a machine take part in a link (PLAN §6.1.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandoverMode {
    /// Every outer edge on that side of the whole desktop.
    #[default]
    All,
    /// Only the first listed monitor that is currently connected.
    Single,
    /// Every listed monitor that is currently connected.
    Selected,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorSelection {
    #[serde(default)]
    pub mode: HandoverMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<MonitorId>,
}

impl MonitorSelection {
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }
    #[must_use]
    pub fn single(id: MonitorId) -> Self {
        Self { mode: HandoverMode::Single, monitors: vec![id] }
    }
    #[must_use]
    pub fn selected(ids: Vec<MonitorId>) -> Self {
        Self { mode: HandoverMode::Selected, monitors: ids }
    }
}

/// How positions map when several monitor edges lead to one neighbour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mapping {
    /// The selected edges form one long edge.
    #[default]
    Continuous,
    /// Each monitor edge maps onto the whole neighbour edge.
    PerMonitor,
}

/// A fraction `[start, end)` of an edge run, `0.0 <= start < end <= 1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

impl Default for Span {
    fn default() -> Self {
        Self::FULL
    }
}

impl Span {
    pub const FULL: Span = Span { start: 0.0, end: 1.0 };

    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.start.is_finite()
            && self.end.is_finite()
            && (0.0..1.0).contains(&self.start)
            && self.end > self.start
            && self.end <= 1.0
    }

    /// Half-open, except that a span ending at 1.0 includes the last pixel.
    #[must_use]
    pub fn contains(&self, t: f64) -> bool {
        t >= self.start && (t < self.end || (self.end >= 1.0 && t <= 1.0))
    }

    /// Global fraction → fraction inside this span.
    #[must_use]
    pub fn to_local(&self, t: f64) -> f64 {
        ((t - self.start) / (self.end - self.start)).clamp(0.0, 1.0)
    }

    /// Fraction inside this span → global fraction.
    #[must_use]
    pub fn to_global(&self, u: f64) -> f64 {
        (self.start + u.clamp(0.0, 1.0) * (self.end - self.start)).clamp(0.0, 1.0)
    }
}

/// One directed connection "leaving `from` through `side` enters `to`".
/// The return path is derived automatically unless an explicit link exists.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkSpec {
    pub from: DeviceId,
    pub to: DeviceId,
    pub side: Side,
    /// Monitors of `from` whose outer edges hand over.
    #[serde(default)]
    pub handover: MonitorSelection,
    #[serde(default)]
    pub mapping: Mapping,
    /// Part of the `from` edge run that hands over.
    #[serde(default)]
    pub from_span: Span,
    /// Monitors of `to` the cursor enters on.
    #[serde(default)]
    pub entry: MonitorSelection,
    /// Part of the `to` edge run the cursor lands on.
    #[serde(default)]
    pub to_span: Span,
}

impl LinkSpec {
    #[must_use]
    pub fn simple(from: DeviceId, side: Side, to: DeviceId) -> Self {
        Self {
            from,
            to,
            side,
            handover: MonitorSelection::all(),
            mapping: Mapping::Continuous,
            from_span: Span::FULL,
            entry: MonitorSelection::all(),
            to_span: Span::FULL,
        }
    }
}
