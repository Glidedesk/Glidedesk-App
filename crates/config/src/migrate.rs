//! Schema migrations. Each step upgrades the raw TOML table by one version,
//! so old files keep their meaning when fields are renamed or moved.

use toml::Table;

use crate::schema::SCHEMA_VERSION;

/// Version stored in the file; files from before versioning count as 1.
#[must_use]
pub fn schema_version(table: &Table) -> u32 {
    table
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v >= 1)
        .unwrap_or(1)
}

/// `STEPS[i]` migrates version `i + 1` to `i + 2`.
const STEPS: &[fn(&mut Table)] = &[v1_to_v2, v2_to_v3];

/// v3: links map per monitor (each screen's edge ↔ the whole other edge, return
/// to the screen you left from). Stretching one long edge over several screens
/// made the cursor jump between screens of different sizes.
fn v2_to_v3(t: &mut Table) {
    if let Some(links) = t.get_mut("layout").and_then(|l| l.get_mut("link")).and_then(toml::Value::as_array_mut) {
        for l in links {
            if let Some(m) = l.get_mut("mapping")
                && m.as_str() == Some("continuous")
            {
                *m = toml::Value::String("per-monitor".into());
            }
        }
    }
}

/// v2: keys behave natively on each computer by default (§14 B8). The old
/// default "auto" (swap Cmd/Ctrl between Mac and PC) becomes "none"; an
/// explicit "swap-ctrl-meta" is kept.
fn v1_to_v2(t: &mut Table) {
    let native = |v: &mut toml::Value| {
        if v.as_str() == Some("auto") {
            *v = toml::Value::String("none".into());
        }
    };
    if let Some(clients) = t.get_mut("server").and_then(|s| s.get_mut("clients")).and_then(toml::Value::as_array_mut) {
        for c in clients {
            if let Some(v) = c.get_mut("key_remap") {
                native(v);
            }
        }
    }
    if let Some(v) = t.get_mut("client").and_then(|c| c.get_mut("key_remap")) {
        native(v);
    }
}

/// Upgrades `table` from `from` to [`SCHEMA_VERSION`].
pub fn upgrade(table: &mut Table, from: u32) {
    for step in STEPS.iter().skip(from.saturating_sub(1) as usize) {
        step(table);
    }
    table.insert("schema_version".into(), toml::Value::Integer(i64::from(SCHEMA_VERSION)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_cover_every_version() {
        assert_eq!(STEPS.len() + 1, SCHEMA_VERSION as usize);
    }

    #[test]
    fn v2_makes_keys_native() {
        let mut t: Table = toml::from_str(
            "[client]\nkey_remap = \"auto\"\n[[server.clients]]\nkey_remap = \"auto\"\n[[server.clients]]\nkey_remap = \"swap-ctrl-meta\"\n",
        )
        .unwrap();
        upgrade(&mut t, 1);
        assert_eq!(t["client"]["key_remap"].as_str(), Some("none"));
        assert_eq!(t["server"]["clients"][0]["key_remap"].as_str(), Some("none"));
        assert_eq!(t["server"]["clients"][1]["key_remap"].as_str(), Some("swap-ctrl-meta"));
    }

    #[test]
    fn v3_maps_links_per_monitor() {
        let mut t: Table = toml::from_str("schema_version = 2\n[[layout.link]]\nmapping = \"continuous\"\n").unwrap();
        upgrade(&mut t, 2);
        assert_eq!(t["layout"]["link"][0]["mapping"].as_str(), Some("per-monitor"));
    }

    #[test]
    fn missing_version_is_one() {
        assert_eq!(schema_version(&Table::new()), 1);
        let mut t = Table::new();
        upgrade(&mut t, 1);
        assert_eq!(schema_version(&t), SCHEMA_VERSION);
    }
}
