//! The semantic index must be something the user asked for.
//!
//! It used to default to on, and on a 776k-file index that meant tens of
//! thousands of files re-read and turned into vectors on every scan, with a
//! status bar that said so for hours. These checks pin the two properties the
//! switch was missing: the default, and that an old config is moved to "off"
//! exactly once.

use kensaku_lib::config::{CURRENT_CONFIG_VERSION, KensakuConfig};

#[test]
fn semantic_index_is_off_in_a_fresh_config() {
    let cfg = KensakuConfig::default();
    assert!(
        !cfg.semantic_enabled,
        "a fresh install must not start building vectors on its own"
    );
}

#[test]
fn semantic_index_stays_off_when_the_field_is_missing() {
    // Configs written before the setting existed have no key at all.
    let cfg = KensakuConfig::load_from_str("{}");
    assert!(!cfg.semantic_enabled);
}

#[test]
fn old_configs_are_moved_to_off_once() {
    let mut old = serde_json::to_value(KensakuConfig::default()).expect("serializes");
    let object = old.as_object_mut().expect("object");
    object.insert("config_version".into(), serde_json::json!(9));
    object.insert("semantic_enabled".into(), serde_json::json!(true));

    let cfg = KensakuConfig::load_from_str(&old.to_string());
    assert_eq!(cfg.config_version, CURRENT_CONFIG_VERSION);
    assert!(
        !cfg.semantic_enabled,
        "upgrading must not keep rebuilding vectors nobody asked for"
    );

    // A user who turns it back on keeps it on: the migration is one-shot.
    let mut after = serde_json::to_value(cfg).expect("serializes");
    after
        .as_object_mut()
        .expect("object")
        .insert("semantic_enabled".into(), serde_json::json!(true));
    let cfg = KensakuConfig::load_from_str(&after.to_string());
    assert!(cfg.semantic_enabled);
}
