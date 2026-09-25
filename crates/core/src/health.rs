//! Client health state machine (PLAN §6.7): heartbeat bookkeeping, latency,
//! Online / Degraded / Locked / Offline.

use std::time::{Duration, Instant};

pub use glidedesk_ipc::views::HealthState;
use glidedesk_proto::ClientStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HealthConfig {
    pub interval: Duration,
    pub miss_threshold: u32,
    pub degraded_latency: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self { interval: Duration::from_secs(1), miss_threshold: 3, degraded_latency: Duration::from_millis(80) }
    }
}

#[derive(Clone, Debug)]
pub struct Health {
    cfg: HealthConfig,
    state: HealthState,
    next_seq: u32,
    last_pong: Option<Instant>,
    connected_at: Option<Instant>,
    /// Smoothed round-trip time.
    rtt: Option<Duration>,
    status: ClientStatus,
}

impl Health {
    #[must_use]
    pub fn new(cfg: HealthConfig) -> Self {
        Self {
            cfg,
            state: HealthState::NeverConnected,
            next_seq: 1,
            last_pong: None,
            connected_at: None,
            rtt: None,
            status: ClientStatus::default(),
        }
    }

    pub fn set_config(&mut self, cfg: HealthConfig) {
        self.cfg = cfg;
    }

    #[must_use]
    pub const fn state(&self) -> HealthState {
        self.state
    }

    #[must_use]
    pub const fn rtt(&self) -> Option<Duration> {
        self.rtt
    }

    #[must_use]
    pub const fn status(&self) -> ClientStatus {
        self.status
    }

    pub fn connecting(&mut self) {
        self.state = HealthState::Connecting;
    }

    /// A new link: nothing measured on an older one (latency, "screen
    /// locked") applies any more — a restarted client reports afresh.
    pub fn connected(&mut self, now: Instant) {
        self.connected_at = Some(now);
        self.last_pong = Some(now);
        self.rtt = None;
        self.status = ClientStatus::default();
        self.state = HealthState::Online;
    }

    /// Link closed or mDNS goodbye: offline immediately.
    pub fn disconnected(&mut self) {
        self.state = HealthState::Offline;
        self.connected_at = None;
    }

    /// Next ping sequence number.
    pub fn next_ping(&mut self) -> u32 {
        let s = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        s
    }

    pub fn pong(&mut self, now: Instant, sent: Instant, status: ClientStatus) {
        let sample = now.saturating_duration_since(sent);
        self.rtt = Some(match self.rtt {
            // EWMA, α = 1/4
            Some(old) => (old * 3 + sample) / 4,
            None => sample,
        });
        self.last_pong = Some(now);
        self.status = status;
        self.state = self.classify();
    }

    fn classify(&self) -> HealthState {
        if self.status.screen_locked || self.status.going_to_sleep || self.status.session_inactive {
            HealthState::Locked
        } else if self.rtt.is_some_and(|r| r > self.cfg.degraded_latency) {
            HealthState::Degraded
        } else {
            HealthState::Online
        }
    }

    /// Heartbeat tick. Returns `true` when the client just became offline
    /// because too many heartbeats were missed.
    pub fn tick(&mut self, now: Instant) -> bool {
        if !self.state.reachable() {
            return false;
        }
        let Some(last) = self.last_pong else { return false };
        let limit = self.cfg.interval * self.cfg.miss_threshold + self.cfg.interval / 2;
        if now.saturating_duration_since(last) > limit {
            self.disconnected();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> HealthConfig {
        HealthConfig::default()
    }

    #[test]
    fn goes_offline_after_missed_heartbeats() {
        let t0 = Instant::now();
        let mut h = Health::new(cfg());
        assert_eq!(h.state(), HealthState::NeverConnected);
        h.connected(t0);
        assert!(!h.tick(t0 + Duration::from_secs(2)));
        assert_eq!(h.state(), HealthState::Online);
        assert!(h.tick(t0 + Duration::from_millis(3600)), "3 missed + grace");
        assert_eq!(h.state(), HealthState::Offline);
        assert!(!h.tick(t0 + Duration::from_secs(10)), "reported once");
    }

    #[test]
    fn pong_resets_and_measures_latency() {
        let t0 = Instant::now();
        let mut h = Health::new(cfg());
        h.connected(t0);
        let sent = t0 + Duration::from_secs(3);
        h.pong(sent + Duration::from_millis(4), sent, ClientStatus::default());
        assert!(!h.tick(t0 + Duration::from_millis(5500)));
        assert_eq!(h.rtt(), Some(Duration::from_millis(4)));
    }

    #[test]
    fn degraded_and_locked() {
        let t0 = Instant::now();
        let mut h = Health::new(cfg());
        h.connected(t0);
        h.pong(t0 + Duration::from_millis(200), t0, ClientStatus::default());
        assert_eq!(h.state(), HealthState::Degraded);
        assert!(h.state().reachable());
        h.pong(
            t0 + Duration::from_millis(201),
            t0 + Duration::from_millis(200),
            ClientStatus { screen_locked: true, ..Default::default() },
        );
        assert_eq!(h.state(), HealthState::Locked);
    }

    #[test]
    fn a_new_link_forgets_what_the_old_one_measured() {
        let t0 = Instant::now();
        let mut h = Health::new(cfg());
        h.connected(t0);
        h.pong(t0 + Duration::from_millis(200), t0, ClientStatus { screen_locked: true, ..Default::default() });
        assert_eq!(h.state(), HealthState::Locked);
        h.disconnected();
        // The client restarted: its old latency and "screen locked" are history.
        h.connected(t0 + Duration::from_secs(5));
        assert_eq!(h.state(), HealthState::Online);
        assert_eq!(h.rtt(), None);
        assert_eq!(h.status(), ClientStatus::default());
    }

    #[test]
    fn disconnect_is_immediate() {
        let mut h = Health::new(cfg());
        h.connected(Instant::now());
        h.disconnected();
        assert_eq!(h.state(), HealthState::Offline);
        assert!(!h.state().reachable());
    }
}
