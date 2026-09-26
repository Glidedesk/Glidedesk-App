//! Nexpingdesk configuration: schema, protected storage, migration,
//! validation and export/import.

#![forbid(unsafe_code)]

pub mod cidr;
pub mod export;
mod merge;
pub mod migrate;
pub mod schema;
pub mod store;
pub mod validate;

pub use cidr::{Cidr, IpFilter};
pub use export::{ExportScope, ImportOptions, ImportPreview, export, import};
pub use schema::*;
pub use store::{APP_DIR_NAME, ConfigError, ConfigStore, LEGACY_APP_DIR_NAME, Loaded, take_over_legacy};
pub use validate::Issue;

#[doc(hidden)]
pub use toml as __toml;
