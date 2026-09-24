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
const STEPS: &[fn(&mut Table)] = &[];

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
    fn missing_version_is_one() {
        assert_eq!(schema_version(&Table::new()), 1);
        let mut t = Table::new();
        upgrade(&mut t, 1);
        assert_eq!(schema_version(&t), SCHEMA_VERSION);
    }
}
