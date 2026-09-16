//! Private helper/master protocol for source-scoped SSH session history and resume.

use crate::ssh_sessions::SshPlatform;
use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};

pub const METHOD: &str = "_intellterm.wta/ssh_sessions";
pub const CHANGED_METHOD: &str = "_intellterm.wta/ssh_sessions/changed";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub target: crate::ssh_sessions::SshTarget,
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    List {
        source: Source,
        // Legacy requests omit POSIX. Windows must be rejected, not silently
        // interpreted as POSIX, by the previous deny_unknown_fields reader.
        #[serde(default, skip_serializing_if = "SshPlatform::is_posix")]
        platform: SshPlatform,
        refresh_history: bool,
    },
    Activate {
        source: Source,
        #[serde(default, skip_serializing_if = "SshPlatform::is_posix")]
        platform: SshPlatform,
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub source: Source,
    #[serde(default)]
    pub platform: SshPlatform,
    #[serde(with = "epoch_serde")]
    pub epoch: uuid::Uuid,
    pub revision: u64,
    pub sessions: Vec<crate::session_registry::SessionInfo>,
}

impl Request {
    pub(crate) fn platform(&self) -> SshPlatform {
        match self {
            Self::List { platform, .. } | Self::Activate { platform, .. } => *platform,
        }
    }
}

pub(crate) fn parse_request(
    raw: &serde_json::value::RawValue,
) -> Result<Request, serde_json::Error> {
    serde_json::from_str(raw.get())
}

pub(crate) fn build_changed_notification(source: &Source) -> acp::schema::v1::ExtNotification {
    let raw = serde_json::value::to_raw_value(source)
        .expect("validated SSH source is trivially serializable");
    acp::schema::v1::ExtNotification::new(CHANGED_METHOD, raw.into())
}

// Keep the existing uuid dependency features unchanged; the wire epoch is a
// canonical UUID string, not a platform-dependent byte representation.
mod epoch_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(epoch: &uuid::Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(epoch)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<uuid::Uuid, D::Error> {
        let text = String::deserialize(deserializer)?;
        uuid::Uuid::parse_str(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
    #[allow(dead_code)]
    enum LegacyRequest {
        List {
            source: Source,
            refresh_history: bool,
        },
        Activate {
            source: Source,
            session_id: String,
        },
    }

    fn source() -> Source {
        Source {
            target: crate::ssh_sessions::SshTarget::new("devbox", Some(2222)).unwrap(),
            agent_id: "copilot".into(),
        }
    }

    #[test]
    fn ssh_request_platform_defaults_and_strict_legacy_reader() {
        for op in ["list", "activate"] {
            let mut wire = json!({ "op": op, "source": source() });
            if op == "list" {
                wire["refresh_history"] = json!(false);
            } else {
                wire["session_id"] = json!("legacy-id");
            }
            let request: Request = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(request.platform(), SshPlatform::Posix);
            assert!(serde_json::from_value::<LegacyRequest>(
                serde_json::to_value(request).unwrap()
            )
            .is_ok());
            wire["platform"] = json!("windows");
            assert_eq!(
                serde_json::from_value::<Request>(wire.clone())
                    .unwrap()
                    .platform(),
                SshPlatform::Windows
            );
            assert!(serde_json::from_value::<LegacyRequest>(wire.clone()).is_err());
            for invalid in [
                json!("auto"),
                json!("Windows"),
                json!("future"),
                json!(null),
                json!(3),
            ] {
                wire["platform"] = invalid;
                assert!(serde_json::from_value::<Request>(wire.clone()).is_err());
            }
            wire["platform"] = json!("posix");
            wire["future_field"] = json!(true);
            assert!(serde_json::from_value::<Request>(wire).is_err());
        }
    }

    #[test]
    fn ssh_snapshot_has_concrete_platform_and_legacy_defaults_are_posix() {
        let snapshot = Snapshot {
            source: source(),
            platform: SshPlatform::Windows,
            epoch: uuid::Uuid::new_v4(),
            revision: 1,
            sessions: Vec::new(),
        };
        let mut wire = serde_json::to_value(snapshot).unwrap();
        assert_eq!(wire["platform"], "windows");
        assert_eq!(
            serde_json::from_value::<Snapshot>(wire.clone())
                .unwrap()
                .platform,
            SshPlatform::Windows
        );
        wire.as_object_mut().unwrap().remove("platform");
        assert_eq!(
            serde_json::from_value::<Snapshot>(wire.clone())
                .unwrap()
                .platform,
            SshPlatform::Posix
        );
        wire["platform"] = json!("auto");
        assert!(serde_json::from_value::<Snapshot>(wire.clone()).is_err());
        wire["platform"] = json!("posix");
        wire["unexpected"] = json!(0);
        assert!(serde_json::from_value::<Snapshot>(wire).is_err());
    }

    #[test]
    fn ssh_platform_is_context_not_source_target_or_location_identity() {
        let mut sources = std::collections::HashSet::new();
        for platform in [SshPlatform::Posix, SshPlatform::Windows] {
            let request = Request::List {
                source: source(),
                platform,
                refresh_history: false,
            };
            let value = serde_json::to_value(request).unwrap();
            sources.insert(serde_json::from_value::<Source>(value["source"].clone()).unwrap());
            assert_eq!(
                value["source"],
                json!({
                    "target": { "destination": "devbox", "port": 2222 }, "agent_id": "copilot"
                })
            );
        }
        assert_eq!(sources.len(), 1);
        let mut value = serde_json::to_value(source()).unwrap();
        value["platform"] = json!("windows");
        assert!(serde_json::from_value::<Source>(value).is_err());
    }
}
