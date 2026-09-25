//! Keeps clients the server learned about while a settings window was open.
//!
//! The server adds a client (with an automatic layout link) the moment it
//! first connects. A settings window that loaded the config earlier doesn't
//! know it yet, so when it saves, its copy has no entry for that client. Only
//! an explicit "Forget" may remove a client; a save that just lacks one keeps it.

use glidedesk_proto::DeviceId;

use crate::schema::Config;

impl Config {
    /// Copies every client of `current` that is missing here and not listed in
    /// `removed`, together with its layout links and tile. Returns the ids kept.
    pub fn keep_known_clients(&mut self, current: &Config, removed: &[DeviceId]) -> Vec<DeviceId> {
        let missing: Vec<DeviceId> = current
            .server
            .clients
            .iter()
            .map(|c| c.id)
            .filter(|id| !removed.contains(id) && self.server.client(id).is_none())
            .collect();
        for id in &missing {
            if let Some(entry) = current.server.client(id) {
                self.server.clients.push(entry.clone());
            }
            let has_link = self.layout.links.iter().any(|l| l.from == *id || l.to == *id);
            if !has_link {
                let links = current.layout.links.iter().filter(|l| l.from == *id || l.to == *id).cloned();
                self.layout.links.extend(links);
            }
            if !self.layout.tiles.iter().any(|t| t.machine == *id)
                && let Some(t) = current.layout.tiles.iter().find(|t| t.machine == *id)
            {
                self.layout.tiles.push(t.clone());
            }
        }
        missing
    }
}

#[cfg(test)]
mod tests {
    use glidedesk_layout::LinkSpec;
    use glidedesk_proto::Side;

    use super::*;
    use crate::schema::{ClientEntry, Tile};

    fn with_client(id: DeviceId, server: DeviceId) -> Config {
        let mut c = Config::default();
        c.device.id = server;
        c.server.clients.push(ClientEntry { id, name: "PC".into(), ..Default::default() });
        c.layout.links.push(LinkSpec::simple(server, Side::Right, id));
        c.layout.tiles.push(Tile { machine: id, x: 10, y: 0 });
        c
    }

    #[test]
    fn a_stale_save_keeps_a_client_that_joined_meanwhile() {
        let (server, pc) = (DeviceId([1; 16]), DeviceId([2; 16]));
        let current = with_client(pc, server);
        let mut stale = Config::default();
        stale.device.id = server;
        stale.general.notifications = false; // the edit the user made
        let kept = stale.keep_known_clients(&current, &[]);
        assert_eq!(kept, vec![pc]);
        assert!(stale.server.client(&pc).is_some());
        assert_eq!(stale.layout.links.len(), 1);
        assert_eq!(stale.layout.tiles.len(), 1);
        assert!(!stale.general.notifications, "the user's edit survives");
    }

    #[test]
    fn forget_removes_and_existing_links_are_not_duplicated() {
        let (server, pc) = (DeviceId([1; 16]), DeviceId([2; 16]));
        let current = with_client(pc, server);
        let mut forgotten = Config::default();
        assert!(forgotten.keep_known_clients(&current, &[pc]).is_empty());
        assert!(forgotten.server.clients.is_empty());

        let mut same = current.clone();
        same.layout.links[0].side = Side::Left; // user moved it
        assert!(same.keep_known_clients(&current, &[]).is_empty());
        assert_eq!(same.layout.links.len(), 1);
        assert_eq!(same.layout.links[0].side, Side::Left);
    }
}
