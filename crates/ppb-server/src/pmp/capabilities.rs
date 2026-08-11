//! PMP capability detection.
//!
//! PMP 1.0.38 verified extended capability set (audit + Owner confirmation):
//! `persist.touches, persist.judges, room.chat_send, stream.touches, stream.judges`.
//! Capability detection is preferred over hardcoding version in business branches
//! (design §11.2, contract §9). Missing capability → `CAPABILITY_NOT_SUPPORTED`.

use std::collections::HashSet;

/// Verified capability set for PMP 1.0.38.
pub const PMP_1_0_38_CAPABILITIES: &[&str] = &[
    "persist.touches",
    "persist.judges",
    "room.chat_send",
    "stream.touches",
    "stream.judges",
];

/// Known capability sets by server version prefix.
pub fn capabilities_for_version(version: &str) -> Vec<String> {
    if version.starts_with("1.0.38") || version == "1.0.38" {
        return PMP_1_0_38_CAPABILITIES
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    // Unknown version: no verified extended capabilities (conservative).
    Vec::new()
}

/// Compute the active capability set given the configured base set and the
/// connected server version.
///
/// - Known version → intersection with the verified map.
/// - Unknown version → the configured base set (operator opted in via config).
pub fn active_capabilities(configured: &[String], server_version: Option<&str>) -> HashSet<String> {
    let base: HashSet<String> = configured.iter().cloned().collect();
    match server_version {
        Some(v) => {
            let known: HashSet<String> = capabilities_for_version(v).into_iter().collect();
            if known.is_empty() {
                base
            } else {
                base.intersection(&known).cloned().collect()
            }
        }
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_138_has_full_set() {
        let caps = capabilities_for_version("1.0.38");
        assert!(caps.contains(&"persist.touches".to_string()));
        assert!(caps.contains(&"stream.judges".to_string()));
        assert_eq!(caps.len(), 5);
    }

    #[test]
    fn unknown_version_is_conservative() {
        assert!(capabilities_for_version("9.9.9").is_empty());
    }

    #[test]
    fn active_intersects_on_known_version() {
        let configured = vec![
            "persist.touches".to_string(),
            "room.chat_send".to_string(),
            "stream.touches".to_string(),
            "future.cap".to_string(),
        ];
        let active = active_capabilities(&configured, Some("1.0.38"));
        assert!(active.contains("persist.touches"));
        assert!(!active.contains("future.cap"));
        assert_eq!(active.len(), 3);
    }

    #[test]
    fn active_uses_config_on_unknown_version() {
        let configured = vec!["persist.touches".to_string()];
        let active = active_capabilities(&configured, Some("9.9.9"));
        assert!(active.contains("persist.touches"));
    }
}
