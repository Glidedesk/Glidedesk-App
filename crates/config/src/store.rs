//! Protected, atomic, versioned storage of `config.toml`.
//!
//! * Unix: directory `0700`, files `0600`, created with those modes (no
//!   window where another user could read them).
//! * Windows: the per-user `%APPDATA%` profile folder is already private to
//!   the user, SYSTEM and Administrators; the agent additionally tightens the
//!   ACL with `glidedesk-platform` at start-up.
//! * Writes go to a temp file in the same directory, are fsync'ed, then
//!   renamed over the old file, so a crash never leaves a half-written config.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use glidedesk_proto::DeviceId;

use crate::migrate;
use crate::schema::{Config, SCHEMA_VERSION};
use crate::validate::Issue;

pub const APP_DIR_NAME: &str = "Glidedesk";
pub const CONFIG_FILE: &str = "config.toml";
const BACKUP_DIR: &str = "backups";
const KEEP_BACKUPS: usize = 5;
/// Refuse to read absurdly large files (hand-edited or hostile).
pub const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config I/O error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("config file is larger than {MAX_CONFIG_BYTES} bytes")]
    TooLarge,
    #[error("config is not valid TOML: {0}")]
    Parse(String),
    #[error("config was written by a newer Glidedesk (schema {found}, this build supports {SCHEMA_VERSION})")]
    Newer { found: u32 },
    #[error("cannot serialise config: {0}")]
    Serialize(String),
    #[error("saving is disabled because the config belongs to a newer version")]
    ReadOnly,
    #[error("no per-user configuration directory could be determined")]
    NoHome,
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ConfigError + '_ {
    move |source| ConfigError::Io { path: path.to_owned(), source }
}

/// What happened while loading.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Values repaired on load; shown once in the UI.
    pub issues: Vec<Issue>,
    pub created: bool,
    pub migrated_from: Option<u32>,
    /// The file was unreadable and was moved aside to this path.
    pub recovered_from: Option<PathBuf>,
    /// Written by a newer version: we run with it but never overwrite it.
    pub read_only: bool,
}

#[derive(Clone, Debug)]
pub struct ConfigStore {
    dir: PathBuf,
    read_only: bool,
}

impl ConfigStore {
    /// Per-user location: macOS `~/Library/Application Support/Glidedesk`,
    /// Windows `%APPDATA%\Glidedesk`, Linux `$XDG_CONFIG_HOME/glidedesk`.
    pub fn default_location() -> Result<Self, ConfigError> {
        // Test / side-by-side instances: everything under $GLIDEDESK_HOME, fully
        // separate from a real installation's settings and control socket.
        if let Some(home) = std::env::var_os("GLIDEDESK_HOME").filter(|h| !h.is_empty()) {
            return Ok(Self::at(PathBuf::from(home).join("config")));
        }
        let base = directories::BaseDirs::new().ok_or(ConfigError::NoHome)?;
        let dir = if cfg!(target_os = "linux") {
            base.config_dir().join(APP_DIR_NAME.to_lowercase())
        } else {
            base.config_dir().join(APP_DIR_NAME)
        };
        Ok(Self::at(dir))
    }

    #[must_use]
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), read_only: false }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE)
    }

    /// Loads the config, creating, migrating or repairing it as needed.
    pub fn load_or_create(&mut self) -> Result<Loaded, ConfigError> {
        create_private_dir(&self.dir)?;
        let path = self.path();
        let text = match read_limited(&path) {
            Ok(t) => t,
            Err(ConfigError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                let mut config = Config::default();
                config.device.id = new_device_id();
                self.save(&config)?;
                return Ok(Loaded {
                    config,
                    issues: Vec::new(),
                    created: true,
                    migrated_from: None,
                    recovered_from: None,
                    read_only: false,
                });
            }
            Err(e) => return Err(e),
        };

        let mut table = match text.parse::<toml::Table>() {
            Ok(t) => t,
            Err(e) => return self.recover(&path, &e.to_string()),
        };
        let found = migrate::schema_version(&table);
        let mut migrated_from = None;
        if found > SCHEMA_VERSION {
            self.read_only = true;
        } else if found < SCHEMA_VERSION {
            self.backup(&format!("before-migrate-v{found}"))?;
            migrate::upgrade(&mut table, found);
            migrated_from = Some(found);
        }

        let mut config: Config = match table.try_into() {
            Ok(c) => c,
            Err(e) => return self.recover(&path, &e.to_string()),
        };
        let mut dirty = migrated_from.is_some();
        if config.device.id == DeviceId([0; 16]) {
            config.device.id = new_device_id();
            dirty = true;
        }
        let issues = config.sanitize();
        dirty |= !issues.is_empty();
        if dirty && !self.read_only {
            self.save(&config)?;
        }
        Ok(Loaded { config, issues, created: false, migrated_from, recovered_from: None, read_only: self.read_only })
    }

    /// Moves an unreadable file aside and starts from defaults.
    fn recover(&mut self, path: &Path, why: &str) -> Result<Loaded, ConfigError> {
        let aside = self.dir.join(format!("config.corrupt-{}.toml", timestamp()));
        fs::rename(path, &aside).map_err(io_err(path))?;
        let mut config = Config::default();
        config.device.id = new_device_id();
        self.save(&config)?;
        Ok(Loaded {
            config,
            issues: vec![Issue {
                key: "config".into(),
                message: format!("the configuration could not be read ({why}); it was saved as {}", aside.display()),
            }],
            created: true,
            migrated_from: None,
            recovered_from: Some(aside),
            read_only: false,
        })
    }

    /// Atomically writes the config with private permissions.
    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        if self.read_only {
            return Err(ConfigError::ReadOnly);
        }
        let text = toml::to_string_pretty(config).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        let body = format!("# Glidedesk configuration — edited by the app; manual edits are kept.\n\n{text}");
        write_atomic(&self.path(), body.as_bytes())
    }

    /// Copies the current file into `backups/`, keeping the newest five.
    pub fn backup(&self, reason: &str) -> Result<Option<PathBuf>, ConfigError> {
        let src = self.path();
        if !src.exists() {
            return Ok(None);
        }
        let dir = self.dir.join(BACKUP_DIR);
        create_private_dir(&dir)?;
        let reason: String = reason.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let dst = dir.join(format!("config-{}-{reason}.toml", timestamp()));
        let data = read_limited(&src)?;
        write_atomic(&dst, data.as_bytes())?;
        prune_backups(&dir)?;
        Ok(Some(dst))
    }

    /// Backs up and replaces the config with defaults (identity is kept).
    pub fn reset(&self, current: &Config) -> Result<Config, ConfigError> {
        self.backup("reset")?;
        let mut fresh = Config::default();
        fresh.device.id = current.device.id;
        fresh.device.name.clone_from(&current.device.name);
        self.save(&fresh)?;
        Ok(fresh)
    }
}

fn timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    let fmt = time::macros::format_description!("[year][month][day]T[hour][minute][second]Z");
    now.format(&fmt).unwrap_or_else(|_| now.unix_timestamp().to_string())
}

/// Random identity from the OS CSPRNG.
#[must_use]
pub fn new_device_id() -> DeviceId {
    let mut b = [0u8; 16];
    // The OS RNG failing is unrecoverable and exceptionally rare; fall back
    // to time-based bytes so the app still starts with a unique-ish id.
    if getrandom::fill(&mut b).is_err() {
        let n = time::OffsetDateTime::now_utc().unix_timestamp_nanos().to_le_bytes();
        b.copy_from_slice(&n);
    }
    if b == [0; 16] {
        b[0] = 1;
    }
    DeviceId(b)
}

fn read_limited(path: &Path) -> Result<String, ConfigError> {
    let f = fs::File::open(path).map_err(io_err(path))?;
    let len = f.metadata().map_err(io_err(path))?.len();
    if len > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge);
    }
    let mut s = String::with_capacity(usize::try_from(len).unwrap_or(0));
    f.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut s).map_err(io_err(path))?;
    Ok(s)
}

/// Creates `dir` (and parents) readable only by the current user.
pub fn create_private_dir(dir: &Path) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(io_err(dir))?;
        // Tighten an existing directory too.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io_err(dir))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    Ok(())
}

/// Write-to-temp + fsync + rename.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), ConfigError> {
    let dir = path.parent().ok_or_else(|| ConfigError::Io {
        path: path.to_owned(),
        source: io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"),
    })?;
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(io_err(&tmp))?;
        f.write_all(data).map_err(io_err(&tmp))?;
        f.sync_all().map_err(io_err(&tmp))?;
        drop(f);
        fs::rename(&tmp, path).map_err(io_err(path))?;
        #[cfg(unix)]
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all(); // best effort: persist the rename
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn prune_backups(dir: &Path) -> Result<(), ConfigError> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    // Names start with a sortable UTC timestamp.
    files.sort();
    let excess = files.len().saturating_sub(KEEP_BACKUPS);
    for old in files.into_iter().take(excess) {
        fs::remove_file(&old).map_err(io_err(&old))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Role;

    #[test]
    fn create_then_reload_keeps_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::at(tmp.path().join("cfg"));
        let first = store.load_or_create().unwrap();
        assert!(first.created);
        assert_ne!(first.config.device.id, DeviceId([0; 16]));
        let again = ConfigStore::at(tmp.path().join("cfg")).load_or_create().unwrap();
        assert!(!again.created);
        assert_eq!(again.config.device.id, first.config.device.id);
    }

    #[cfg(unix)]
    #[test]
    fn files_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::at(tmp.path().join("cfg"));
        store.load_or_create().unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(store.dir()), 0o700);
        assert_eq!(mode(&store.path()), 0o600);
    }

    #[test]
    fn save_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::at(tmp.path());
        let mut cfg = store.load_or_create().unwrap().config;
        cfg.device.role = Role::Server;
        cfg.server.network.port = 30_000;
        store.save(&cfg).unwrap();
        let back = ConfigStore::at(tmp.path()).load_or_create().unwrap().config;
        assert_eq!(back, cfg);
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(CONFIG_FILE), "this is = = not toml").unwrap();
        let loaded = ConfigStore::at(tmp.path()).load_or_create().unwrap();
        let aside = loaded.recovered_from.expect("moved aside");
        assert!(aside.exists());
        assert_eq!(loaded.issues.len(), 1);
    }

    #[test]
    fn newer_schema_is_read_only() {
        let tmp = tempfile::tempdir().unwrap();
        let text = format!("schema_version = {}\n[device]\nid = \"{}\"\n", SCHEMA_VERSION + 1, "ab".repeat(16));
        fs::write(tmp.path().join(CONFIG_FILE), &text).unwrap();
        let mut store = ConfigStore::at(tmp.path());
        let loaded = store.load_or_create().unwrap();
        assert!(loaded.read_only);
        assert!(matches!(store.save(&loaded.config), Err(ConfigError::ReadOnly)));
        assert_eq!(fs::read_to_string(tmp.path().join(CONFIG_FILE)).unwrap(), text, "never overwritten");
    }

    #[test]
    fn repairs_are_saved_and_unknown_keys_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let text = format!(
            "schema_version = 1\nfuture_option = true\n[device]\nid = \"{}\"\n[server.health]\ninterval_ms = 5\n",
            "cd".repeat(16)
        );
        fs::write(tmp.path().join(CONFIG_FILE), text).unwrap();
        let loaded = ConfigStore::at(tmp.path()).load_or_create().unwrap();
        assert_eq!(loaded.config.server.health.interval_ms, 500);
        assert!(!loaded.issues.is_empty());
        let again = ConfigStore::at(tmp.path()).load_or_create().unwrap();
        assert!(again.issues.is_empty(), "repair was persisted");
    }

    #[test]
    fn backups_are_pruned() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::at(tmp.path());
        store.load_or_create().unwrap();
        for i in 0..8 {
            // Distinct names without sleeping: vary the reason.
            store.backup(&format!("r{i}")).unwrap();
        }
        let n = fs::read_dir(tmp.path().join(BACKUP_DIR)).unwrap().count();
        assert!(n <= KEEP_BACKUPS, "{n} backups kept");
    }

    #[test]
    fn oversized_file_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let f = fs::File::create(tmp.path().join(CONFIG_FILE)).unwrap();
        f.set_len(MAX_CONFIG_BYTES + 1).unwrap();
        assert!(matches!(ConfigStore::at(tmp.path()).load_or_create(), Err(ConfigError::TooLarge)));
    }
}
