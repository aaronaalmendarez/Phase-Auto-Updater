//! Local install preference/receipt. Authorization always comes from the server.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseChannel {
    #[default]
    Stable,
    EarlyAccess,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRelease {
    pub channel: ReleaseChannel,
    pub build_id: String,
    pub sha256: String,
    pub path: PathBuf,
}

impl InstalledRelease {
    pub fn matches(&self, path: &std::path::Path, build_id: &str, actual_hash: &str) -> bool {
        self.path == path && self.build_id == build_id && self.sha256 == actual_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_is_specific_to_file_build_and_path() {
        let receipt = InstalledRelease {
            channel: ReleaseChannel::EarlyAccess,
            build_id: "ea-1".into(),
            sha256: "customer-a".into(),
            path: PathBuf::from("plugin.rbxm"),
        };
        assert!(receipt.matches(std::path::Path::new("plugin.rbxm"), "ea-1", "customer-a"));
        assert!(!receipt.matches(std::path::Path::new("plugin.rbxm"), "ea-1", "customer-b"));
        assert!(!receipt.matches(std::path::Path::new("plugin.rbxm"), "ea-2", "customer-a"));
        assert!(!receipt.matches(std::path::Path::new("other.rbxm"), "ea-1", "customer-a"));
    }
}
