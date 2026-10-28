//! Registers the adapters shipped inside the app.
//!
//! The Paperclip server discovers external adapters through
//! `$PAPERCLIP_HOME/adapter-plugins.json`, which is normally written by an
//! operator installing a plugin. Adapters we bundle have to end up in that same
//! file, otherwise they exist on disk but are never loaded — so the shell
//! reconciles the file against the bundle on every launch.
//!
//! Every launch, not just the first, because the entry stores an absolute
//! `localPath`: the moment someone drags the app from `~/Downloads` to
//! `/Applications`, a first-run-only entry would point at a directory that no
//! longer exists and the adapter would silently stop loading.
//!
//! The reconciliation is deliberately narrow. It only touches records whose
//! `packageName` it owns, and it preserves the rest of each record verbatim —
//! most importantly `disabled`, so an adapter the user turned off in the UI
//! does not come back at the next launch.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Written into the server bundle by `scripts/bundle-adapters.mjs`.
const MANIFEST_FILE: &str = "paperclip-bundled-adapters.json";
const STORE_FILE: &str = "adapter-plugins.json";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundledAdapter {
    pub package_name: String,
    #[serde(rename = "type")]
    pub adapter_type: String,
}

/// A record in `adapter-plugins.json`.
///
/// Unknown fields are captured and written back untouched: the server owns this
/// schema and may add to it, and dropping a field we did not know about would
/// quietly discard state the server depends on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AdapterRecord {
    pub package_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    #[serde(rename = "type")]
    pub adapter_type: String,
    pub installed_at: String,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, serde_json::Value>,
}

pub fn manifest_path(app_root: &Path) -> PathBuf {
    app_root.join("server").join(MANIFEST_FILE)
}

pub fn store_path(paperclip_home: &Path) -> PathBuf {
    paperclip_home.join(STORE_FILE)
}

/// Where a bundled package lives inside the app.
///
/// Adapters are staged into the server's own flat `node_modules` so their
/// dependencies resolve from the packages the server already ships.
pub fn bundled_package_dir(app_root: &Path, package_name: &str) -> PathBuf {
    let mut dir = app_root.join("server").join("node_modules");
    for segment in package_name.split('/') {
        dir = dir.join(segment);
    }
    dir
}

fn read_manifest(path: &Path) -> Vec<BundledAdapter> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        log::warn!("ignoring malformed bundled-adapter manifest at {path:?}: {error}");
        Vec::new()
    })
}

fn read_store(path: &Path) -> Vec<AdapterRecord> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_else(|error| {
        // Refusing to parse is safer than overwriting: the file may hold
        // adapters the user installed by hand.
        log::warn!("could not parse {path:?} ({error}); leaving existing records alone");
        Vec::new()
    })
}

/// Merges the bundled adapters into `records`, returning true if anything changed.
pub fn reconcile(
    records: &mut Vec<AdapterRecord>,
    bundled: &[BundledAdapter],
    app_root: &Path,
    now: &str,
) -> bool {
    let mut changed = false;

    for adapter in bundled {
        let local_path = bundled_package_dir(app_root, &adapter.package_name)
            .to_string_lossy()
            .into_owned();

        match records
            .iter_mut()
            .find(|record| record.package_name == adapter.package_name)
        {
            Some(existing) => {
                // Only the path and type are ours to refresh; `disabled` and
                // anything else the server wrote stays as it is.
                if existing.local_path.as_deref() != Some(local_path.as_str()) {
                    existing.local_path = Some(local_path);
                    changed = true;
                }
                if existing.adapter_type != adapter.adapter_type {
                    existing.adapter_type = adapter.adapter_type.clone();
                    changed = true;
                }
            }
            None => {
                records.push(AdapterRecord {
                    package_name: adapter.package_name.clone(),
                    local_path: Some(local_path),
                    adapter_type: adapter.adapter_type.clone(),
                    installed_at: now.to_string(),
                    rest: serde_json::Map::new(),
                });
                changed = true;
            }
        }
    }

    changed
}

/// Reconciles the on-disk store with the adapters shipped in this build.
///
/// Failures are logged and swallowed: a missing adapter degrades the app to
/// "the built-in adapters work", which is far better than refusing to start.
pub fn register_bundled(app_root: &Path, paperclip_home: &Path) {
    let bundled = read_manifest(&manifest_path(app_root));
    if bundled.is_empty() {
        return;
    }

    let store = store_path(paperclip_home);
    let mut records = read_store(&store);

    if !reconcile(
        &mut records,
        &bundled,
        app_root,
        &crate::connection::profiles::now_iso8601(),
    ) {
        return;
    }

    if let Err(error) = std::fs::create_dir_all(paperclip_home) {
        log::warn!("could not create {paperclip_home:?} for the adapter store: {error}");
        return;
    }

    match serde_json::to_string_pretty(&records) {
        Ok(json) => {
            if let Err(error) = std::fs::write(&store, format!("{json}\n")) {
                log::warn!("could not write {store:?}: {error}");
            } else {
                log::info!(
                    "registered {} bundled adapter(s): {}",
                    bundled.len(),
                    bundled
                        .iter()
                        .map(|adapter| adapter.adapter_type.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        Err(error) => log::warn!("could not serialise the adapter store: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled() -> Vec<BundledAdapter> {
        vec![BundledAdapter {
            package_name: "paperclip-adapter-devin".into(),
            adapter_type: "devin_local".into(),
        }]
    }

    fn record(json: &str) -> AdapterRecord {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_bundled_adapter_is_added_to_an_empty_store() {
        let mut records = Vec::new();
        let changed = reconcile(&mut records, &bundled(), Path::new("/App"), "now");

        assert!(changed);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].adapter_type, "devin_local");
        assert_eq!(
            records[0].local_path.as_deref(),
            Some("/App/server/node_modules/paperclip-adapter-devin")
        );
    }

    /// The whole reason this runs on every launch: moving the app invalidates
    /// the stored absolute path.
    #[test]
    fn a_stale_path_is_refreshed_when_the_app_moves() {
        let mut records = vec![record(
            r#"{"packageName":"paperclip-adapter-devin","localPath":"/Volumes/Install/server/node_modules/paperclip-adapter-devin","type":"devin_local","installedAt":"then"}"#,
        )];

        assert!(reconcile(
            &mut records,
            &bundled(),
            Path::new("/Applications/P.app"),
            "now"
        ));
        assert_eq!(
            records[0].local_path.as_deref(),
            Some("/Applications/P.app/server/node_modules/paperclip-adapter-devin")
        );
        assert_eq!(records[0].installed_at, "then", "must not be re-stamped");
    }

    #[test]
    fn nothing_is_written_when_the_store_is_already_correct() {
        let mut records = vec![record(
            r#"{"packageName":"paperclip-adapter-devin","localPath":"/App/server/node_modules/paperclip-adapter-devin","type":"devin_local","installedAt":"then"}"#,
        )];

        assert!(!reconcile(
            &mut records,
            &bundled(),
            Path::new("/App"),
            "now"
        ));
    }

    /// An adapter switched off in the UI must stay off.
    #[test]
    fn a_disabled_adapter_is_not_re_enabled() {
        let mut records = vec![record(
            r#"{"packageName":"paperclip-adapter-devin","localPath":"/App/server/node_modules/paperclip-adapter-devin","type":"devin_local","installedAt":"then","disabled":true}"#,
        )];

        reconcile(&mut records, &bundled(), Path::new("/App"), "now");
        assert_eq!(
            records[0].rest.get("disabled"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn adapters_the_user_installed_are_left_untouched() {
        let mut records = vec![record(
            r#"{"packageName":"some-other-adapter","localPath":"/home/me/other","type":"other","installedAt":"then"}"#,
        )];

        reconcile(&mut records, &bundled(), Path::new("/App"), "now");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].package_name, "some-other-adapter");
        assert_eq!(records[0].local_path.as_deref(), Some("/home/me/other"));
    }

    #[test]
    fn unknown_fields_survive_a_rewrite() {
        let mut records = vec![record(
            r#"{"packageName":"paperclip-adapter-devin","localPath":"/old","type":"devin_local","installedAt":"then","futureField":{"a":1}}"#,
        )];

        reconcile(&mut records, &bundled(), Path::new("/App"), "now");
        let json = serde_json::to_string(&records[0]).unwrap();

        assert!(json.contains("futureField"), "serialised as: {json}");
    }

    #[test]
    fn a_scoped_package_maps_to_a_nested_directory() {
        assert_eq!(
            bundled_package_dir(Path::new("/App"), "@acme/adapter-x"),
            Path::new("/App/server/node_modules/@acme/adapter-x")
        );
    }
}
