//! Metadata reported over an explicitly selected companion protocol, not an
//! attestation of hardware capability or a guessed identity from a USB name.
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct HelperBuild {
    pub component: String,
    pub package_version: String,
    pub git_revision: Option<String>,
    pub source_tag: Option<String>,
    pub tracked_source_modified: Option<bool>,
    pub display_version: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Image {
    pub version: u16,
    pub build_version: u16,
    pub build_time: String,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadState {
    NotInitialized,
    RunningImageUnverified,
    UploadedThisInitialization,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Firmware {
    pub supplied_image: Image,
    pub load_state: LoadState,
    pub uploaded_image: Option<Image>,
    pub evidence: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Identity {
    pub helper_build: Option<HelperBuild>,
    pub firmware: Option<Firmware>,
}
fn invalid() -> io::Error {
    io::Error::other("invalid companion identity metadata")
}
fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn hash(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}
impl Image {
    fn valid(&self) -> bool {
        self.build_time.len() <= 32
            && !self.build_time.chars().any(char::is_control)
            && hash(&self.sha256, 64)
    }
}
impl Identity {
    pub fn from_hello(value: &serde_json::Value) -> io::Result<Self> {
        let helper_build: Option<HelperBuild> = value
            .get("helper_build")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::from_value(v.clone()).map_err(|_| invalid()))
            .transpose()?;
        if helper_build.as_ref().is_some_and(|b| {
            b.component != "mt7610u-lab"
                || !text(&b.package_version, 64)
                || !text(&b.display_version, 256)
                || b.git_revision.as_ref().is_some_and(|s| !hash(s, 40))
                || b.source_tag.as_ref().is_some_and(|s| !text(s, 128))
        }) {
            return Err(invalid());
        }
        Ok(Self {
            helper_build,
            firmware: None,
        })
    }
    /// Every accepted status replaces firmware state. Omission by an older helper
    /// means unknown; recovering/disconnected states cannot retain an uploaded image.
    pub fn status(&mut self, value: &serde_json::Value) -> io::Result<()> {
        self.firmware = None;
        let firmware: Option<Firmware> = value
            .get("firmware")
            .filter(|v| !v.is_null())
            .map(|v| serde_json::from_value(v.clone()).map_err(|_| invalid()))
            .transpose()?;
        if let Some(f) = &firmware {
            let uploaded = f.load_state == LoadState::UploadedThisInitialization;
            if !f.supplied_image.valid()
                || f.evidence != "companion_bootstrap"
                || uploaded != f.uploaded_image.is_some()
                || f.uploaded_image
                    .as_ref()
                    .is_some_and(|image| image != &f.supplied_image)
                || (uploaded && value["state"] != "running")
            {
                return Err(invalid());
            }
        }
        self.firmware = firmware;
        Ok(())
    }
    pub fn description(&self) -> String {
        let helper = self
            .helper_build
            .as_ref()
            .map(|b| {
                format!(
                    "mt7610u-lab package {} · {}",
                    b.package_version, b.display_version
                )
            })
            .unwrap_or_else(|| "MT7610U helper build unreported".into());
        let firmware = self
            .firmware
            .as_ref()
            .and_then(|f| f.uploaded_image.as_ref())
            .map(|f| {
                format!(
                    "uploaded firmware image {} / build {} ({})",
                    f.version, f.build_version, f.build_time
                )
            })
            .unwrap_or_else(|| "running firmware identity unverified".into());
        format!("{helper}; {firmware}")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn metadata_distinguishes_supplied_uploaded_and_reused_images() {
        let mut identity =
            Identity::from_hello(&json!({"type":"hello","protocol_version":1})).unwrap();
        assert!(identity.helper_build.is_none());
        let image = json!({"version":123,"build_version":4,"build_time":"20130101","sha256":"a".repeat(64)});
        let mut status = json!({"state":"running","firmware":{"supplied_image":image,"load_state":"uploaded_this_initialization","uploaded_image":image,"evidence":"companion_bootstrap"}});
        identity.status(&status).unwrap();
        assert!(identity
            .description()
            .contains("uploaded firmware image 123"));
        status["state"] = json!("recovering");
        assert!(identity.status(&status).is_err());
        assert!(identity.firmware.is_none());
        status["firmware"]["load_state"] = json!("running_image_unverified");
        status["firmware"]["uploaded_image"] = serde_json::Value::Null;
        identity.status(&status).unwrap();
        assert!(identity.description().contains("unverified"));
        identity.status(&json!({"state":"running"})).unwrap();
        assert!(identity.firmware.is_none());
    }
    #[test]
    fn companion_strings_are_bounded_and_unknown_keys_cannot_escape() {
        let mut hello = json!({"helper_build":{"component":"mt7610u-lab","package_version":"0.1.0","git_revision":"a".repeat(40),"source_tag":null,"tracked_source_modified":false,"display_version":"git aaaaaaaaaaaa","location":{"lat":1}}});
        let identity = Identity::from_hello(&hello).unwrap();
        assert!(serde_json::to_value(identity).unwrap()["helper_build"]
            .get("location")
            .is_none());
        hello["helper_build"]["display_version"] = json!("x".repeat(257));
        assert!(Identity::from_hello(&hello).is_err());
        hello["helper_build"]["display_version"] = json!("bad\nlabel");
        assert!(Identity::from_hello(&hello).is_err());
    }
}
