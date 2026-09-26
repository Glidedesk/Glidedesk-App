//! Export / import of settings.
//!
//! Exports never contain this machine's identity (`device.id`), so importing
//! a file on another computer does not clone its identity.

use toml::{Table, Value};

use crate::migrate;
use crate::schema::{Config, SCHEMA_VERSION};
use crate::store::MAX_CONFIG_BYTES;
use crate::validate::Issue;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportScope {
    /// Everything except the machine identity.
    Full,
    /// Layout and the per-client settings it refers to.
    LayoutOnly,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("file is larger than {MAX_CONFIG_BYTES} bytes")]
    TooLarge,
    #[error("not a valid Nexpingdesk settings file: {0}")]
    Invalid(String),
    #[error("file comes from a newer Nexpingdesk (schema {0})")]
    Newer(u32),
}

/// Serialises `cfg` for sharing or backup.
pub fn export(cfg: &Config, scope: ExportScope, app_version: &str) -> Result<String, ExportError> {
    let mut table = Table::try_from(cfg).map_err(|e| ExportError::Invalid(e.to_string()))?;
    if let Some(Value::Table(dev)) = table.get_mut("device") {
        dev.remove("id");
    }
    // Passwords never leave this computer in an export (§14.1).
    if let Some(Value::Table(net)) = table.get_mut("server").and_then(|s| s.get_mut("network")) {
        net.remove("password");
    }
    if let Some(Value::Table(client)) = table.get_mut("client") {
        client.remove("password");
    }
    if scope == ExportScope::LayoutOnly {
        let server_clients =
            table.get("server").and_then(|s| s.get("clients")).cloned().unwrap_or(Value::Array(Vec::new()));
        let layout = table.remove("layout").unwrap_or(Value::Table(Table::new()));
        let mut server = Table::new();
        server.insert("clients".into(), server_clients);
        table = Table::new();
        table.insert("schema_version".into(), Value::Integer(i64::from(SCHEMA_VERSION)));
        table.insert("server".into(), Value::Table(server));
        table.insert("layout".into(), layout);
    }
    let mut meta = Table::new();
    meta.insert("app_version".into(), Value::String(app_version.to_owned()));
    meta.insert("scope".into(), Value::String(if scope == ExportScope::Full { "full" } else { "layout" }.into()));
    table.insert("export".into(), Value::Table(meta));
    let body = toml::to_string_pretty(&table).map_err(|e| ExportError::Invalid(e.to_string()))?;
    Ok(format!("# Nexpingdesk settings export\n\n{body}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    /// Keep this machine's network interface/IP selection (they rarely exist
    /// on another computer).
    pub keep_network: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self { keep_network: true }
    }
}

/// Result of reading an import file; nothing is applied until the caller saves `config`.
#[derive(Debug)]
pub struct ImportPreview {
    pub config: Config,
    pub layout_only: bool,
    pub issues: Vec<Issue>,
    /// Top-level sections that differ from the current config.
    pub changed_sections: Vec<&'static str>,
}

/// Parses and validates an exported (or plain `config.toml`) file.
pub fn import(text: &str, current: &Config, opts: ImportOptions) -> Result<ImportPreview, ExportError> {
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ExportError::TooLarge);
    }
    let mut table: Table = text.parse().map_err(|e: toml::de::Error| ExportError::Invalid(e.to_string()))?;
    let layout_only =
        table.get("export").and_then(|m| m.get("scope")).and_then(Value::as_str).is_some_and(|s| s == "layout");
    table.remove("export");
    let version = migrate::schema_version(&table);
    if version > SCHEMA_VERSION {
        return Err(ExportError::Newer(version));
    }
    migrate::upgrade(&mut table, version);
    let imported: Config = table.try_into().map_err(|e: toml::de::Error| ExportError::Invalid(e.to_string()))?;

    let mut config = if layout_only {
        let mut c = current.clone();
        c.layout = imported.layout;
        for entry in imported.server.clients {
            match c.server.clients.iter_mut().find(|e| e.id == entry.id) {
                Some(existing) => *existing = entry,
                None => c.server.clients.push(entry),
            }
        }
        c
    } else {
        let mut c = imported;
        c.device.id = current.device.id;
        if c.device.name.is_empty() {
            c.device.name.clone_from(&current.device.name);
        }
        // Imports never set or clear a password.
        c.server.network.password.clone_from(&current.server.network.password);
        c.client.password.clone_from(&current.client.password);
        if opts.keep_network {
            let net = &current.server.network;
            c.server.network.mode = net.mode;
            c.server.network.interfaces.clone_from(&net.interfaces);
            c.server.network.addresses.clone_from(&net.addresses);
            c.client.interface.clone_from(&current.client.interface);
        }
        c
    };
    let issues = config.sanitize();
    let mut changed_sections = Vec::new();
    if config.device != current.device {
        changed_sections.push("device");
    }
    if config.general != current.general {
        changed_sections.push("general");
    }
    if config.server != current.server {
        changed_sections.push("server");
    }
    if config.client != current.client {
        changed_sections.push("client");
    }
    if config.layout != current.layout {
        changed_sections.push("layout");
    }
    Ok(ImportPreview { config, layout_only, issues, changed_sections })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{BindMode, ClientEntry, Role};
    use nexpingdesk_layout::LinkSpec;
    use nexpingdesk_proto::{DeviceId, Side};

    fn sample() -> Config {
        let mut c = Config::default();
        c.device.id = DeviceId([9; 16]);
        c.device.role = Role::Server;
        c.server.network.mode = BindMode::Interfaces;
        c.server.network.interfaces = vec!["en0".into()];
        c.server.clients.push(ClientEntry { id: DeviceId([2; 16]), name: "pc".into(), ..Default::default() });
        c.layout.links.push(LinkSpec::simple(DeviceId([9; 16]), Side::Right, DeviceId([2; 16])));
        c
    }

    #[test]
    fn export_omits_passwords_and_import_keeps_local_ones() {
        let mut c = Config::default();
        c.server.network.password = Some(crate::StoredPassword { salt: "aa".repeat(16), key: "bb".repeat(32) });
        c.client.password = "hunter2".into();
        let text = export(&c, ExportScope::Full, "1").unwrap();
        assert!(!text.contains("hunter2") && !text.contains(&"bb".repeat(32)), "{text}");
        let preview = import(&text, &c, ImportOptions::default()).unwrap();
        assert_eq!(preview.config.client.password, "hunter2");
        assert!(preview.config.server.network.password.is_some());
    }

    #[test]
    fn export_omits_identity() {
        let text = export(&sample(), ExportScope::Full, "1.0.0").unwrap();
        let table: Table = text.parse().unwrap();
        assert!(table["device"].get("id").is_none(), "device id leaked:\n{text}");
    }

    #[test]
    fn full_roundtrip_keeps_local_identity_and_network() {
        let text = export(&sample(), ExportScope::Full, "1.0.0").unwrap();
        let mut other = Config::default();
        other.device.id = DeviceId([5; 16]);
        other.server.network.interfaces = vec!["eth7".into()];
        let preview = import(&text, &other, ImportOptions::default()).unwrap();
        assert_eq!(preview.config.device.id, DeviceId([5; 16]));
        assert_eq!(preview.config.server.network.interfaces, vec!["eth7".to_string()]);
        assert_eq!(preview.config.layout, sample().layout);
        assert!(preview.changed_sections.contains(&"layout"));
    }

    #[test]
    fn layout_only_merges() {
        let text = export(&sample(), ExportScope::LayoutOnly, "1.0.0").unwrap();
        let mut other = Config::default();
        other.general.notifications = false;
        let preview = import(&text, &other, ImportOptions::default()).unwrap();
        assert!(preview.layout_only);
        assert!(!preview.config.general.notifications, "other sections untouched");
        assert_eq!(preview.config.server.clients.len(), 1);
    }

    #[test]
    fn rejects_bad_and_newer_files() {
        assert!(matches!(import("= nope", &Config::default(), ImportOptions::default()), Err(ExportError::Invalid(_))));
        let newer = format!("schema_version = {}\n", SCHEMA_VERSION + 3);
        assert!(matches!(import(&newer, &Config::default(), ImportOptions::default()), Err(ExportError::Newer(_))));
        let huge = "#".repeat(usize::try_from(MAX_CONFIG_BYTES).unwrap() + 1);
        assert!(matches!(import(&huge, &Config::default(), ImportOptions::default()), Err(ExportError::TooLarge)));
    }
}
