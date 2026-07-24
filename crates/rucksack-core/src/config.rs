use crate::files::{atomic_write_toml, read_toml};
use crate::paths::AppPaths;
use crate::policy::Focus;
use crate::protocol::MAX_HELPER_TTL_SECONDS;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub default_agent: Option<crate::agent::AgentKind>,
    pub hotspot: HotspotConfig,
    pub session: SessionConfig,
    pub safety: SafetyConfig,
    pub adapters: AdapterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotspotConfig {
    pub ssid: Option<String>,
    pub require_verified_ssid: bool,
    pub require_iphone_usb: bool,
    pub probe_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    pub duration_minutes: u64,
    pub focus: Focus,
    pub heartbeat_seconds: u64,
    pub helper_ttl_seconds: u64,
    pub network_outage_grace_seconds: u64,
    pub idle_grace_seconds: u64,
    #[serde(alias = "stop_owned_remote_on_arrive")]
    pub stop_owned_remote_on_unpack: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SafetyConfig {
    pub minimum_start_battery_percent: u8,
    pub warn_battery_percent: u8,
    pub sleep_battery_percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdapterConfig {
    pub codex: bool,
    pub claude: bool,
    pub cursor: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            default_agent: None,
            hotspot: HotspotConfig::default(),
            session: SessionConfig::default(),
            safety: SafetyConfig::default(),
            adapters: AdapterConfig::default(),
        }
    }
}

impl Default for HotspotConfig {
    fn default() -> Self {
        Self {
            ssid: None,
            require_verified_ssid: true,
            require_iphone_usb: false,
            probe_timeout_seconds: 6,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            duration_minutes: 24 * 60,
            focus: Focus::Continue,
            heartbeat_seconds: 30,
            helper_ttl_seconds: 90,
            network_outage_grace_seconds: 5 * 60,
            idle_grace_seconds: 15 * 60,
            stop_owned_remote_on_unpack: false,
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            minimum_start_battery_percent: 35,
            warn_battery_percent: 20,
            sleep_battery_percent: 15,
        }
    }
}

impl Default for AdapterConfig {
    fn default() -> Self {
        Self {
            codex: true,
            claude: true,
            cursor: true,
        }
    }
}

impl Config {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        Self::load_from(&paths.config_file)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if path.exists() {
            read_toml(path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        atomic_write_toml(&paths.config_file, self)
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.version != CONFIG_VERSION {
            return Err(format!(
                "Unsupported config version {}; expected {}",
                self.version, CONFIG_VERSION
            ));
        }
        let start = self.safety.minimum_start_battery_percent;
        let warn = self.safety.warn_battery_percent;
        let floor = self.safety.sleep_battery_percent;
        if start > 100 || warn > 100 || floor > 100 {
            return Err("Battery thresholds must be between 0 and 100".to_owned());
        }
        if floor >= warn {
            return Err("sleep_battery_percent must be lower than warn_battery_percent".to_owned());
        }
        if warn >= start {
            return Err(
                "warn_battery_percent must be lower than minimum_start_battery_percent".to_owned(),
            );
        }
        if self.hotspot.probe_timeout_seconds == 0 || self.hotspot.probe_timeout_seconds > 30 {
            return Err("probe_timeout_seconds must be between 1 and 30".to_owned());
        }
        if self
            .hotspot
            .ssid
            .as_deref()
            .is_some_and(|ssid| ssid.trim().is_empty() || ssid.chars().any(char::is_control))
        {
            return Err(
                "hotspot.ssid must contain a non-empty network name without control characters"
                    .to_owned(),
            );
        }
        if self.hotspot.require_iphone_usb
            && (self.hotspot.require_verified_ssid || self.hotspot.ssid.is_some())
        {
            return Err(
                "require_iphone_usb cannot be combined with a verified Wi-Fi hotspot".to_owned(),
            );
        }
        if self.session.heartbeat_seconds == 0 {
            return Err("heartbeat_seconds must be greater than zero".to_owned());
        }
        if self.session.helper_ttl_seconds > MAX_HELPER_TTL_SECONDS {
            return Err(format!(
                "helper_ttl_seconds must not exceed the helper protocol maximum of {MAX_HELPER_TTL_SECONDS}"
            ));
        }
        let minimum_ttl = self
            .session
            .heartbeat_seconds
            .checked_mul(2)
            .ok_or_else(|| "heartbeat_seconds is too large".to_owned())?;
        if self.session.helper_ttl_seconds < minimum_ttl {
            return Err("helper_ttl_seconds must be at least twice heartbeat_seconds".to_owned());
        }
        let minimum_network_grace = self
            .session
            .heartbeat_seconds
            .checked_mul(2)
            .ok_or_else(|| "heartbeat_seconds is too large".to_owned())?;
        if self.session.network_outage_grace_seconds < minimum_network_grace
            || self.session.network_outage_grace_seconds > 15 * 60
        {
            return Err(
                "network_outage_grace_seconds must be at least twice heartbeat_seconds and no more than 900"
                    .to_owned(),
            );
        }
        if self.session.idle_grace_seconds == 0 || self.session.idle_grace_seconds > 60 * 60 {
            return Err("idle_grace_seconds must be between 1 and 3600".to_owned());
        }
        if self.session.duration_minutes == 0 || self.session.duration_minutes > 24 * 60 {
            return Err("duration_minutes must be between 1 and 1440".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();

        assert!(config.validate().is_ok());
        assert_eq!(config.session.duration_minutes, 24 * 60);
    }

    #[test]
    fn legacy_arrive_config_key_loads_and_serializes_as_unpack() {
        let current = toml::to_string_pretty(&Config::default()).unwrap();
        let legacy = current.replace("stop_owned_remote_on_unpack", "stop_owned_remote_on_arrive");

        let parsed = toml::from_str::<Config>(&legacy).unwrap();
        let serialized = toml::to_string_pretty(&parsed).unwrap();

        assert!(!parsed.session.stop_owned_remote_on_unpack);
        assert!(serialized.contains("stop_owned_remote_on_unpack"));
        assert!(!serialized.contains("stop_owned_remote_on_arrive"));
    }

    #[test]
    fn rejects_helper_ttl_above_protocol_limit() {
        let mut config = Config::default();
        config.session.helper_ttl_seconds = MAX_HELPER_TTL_SECONDS + 1;
        assert!(config.validate().unwrap_err().contains("protocol maximum"));
    }

    #[test]
    fn rejects_duration_over_one_day() {
        let mut config = Config::default();
        config.session.duration_minutes = 24 * 60 + 1;
        assert!(config.validate().unwrap_err().contains("1440"));
    }

    #[test]
    fn rejects_zero_probe_timeout() {
        let mut config = Config::default();
        config.hotspot.probe_timeout_seconds = 0;
        assert!(config
            .validate()
            .unwrap_err()
            .contains("probe_timeout_seconds"));
    }

    #[test]
    fn rejects_empty_hotspots() {
        let mut config = Config::default();
        config.hotspot.ssid = Some("  ".to_owned());
        assert!(config.validate().unwrap_err().contains("hotspot.ssid"));
    }

    #[test]
    fn rejects_hotspots_with_control_characters() {
        let mut config = Config::default();
        config.hotspot.ssid = Some("Noah\nforged".to_owned());
        assert!(config
            .validate()
            .unwrap_err()
            .contains("control characters"));
    }

    #[test]
    fn rejects_conflicting_wifi_and_usb_requirements() {
        let mut config = Config::default();
        config.hotspot.require_iphone_usb = true;
        assert!(config
            .validate()
            .unwrap_err()
            .contains("require_iphone_usb"));
    }

    #[test]
    fn rejects_network_grace_shorter_than_two_heartbeats() {
        let mut config = Config::default();
        config.session.network_outage_grace_seconds = config.session.heartbeat_seconds * 2 - 1;
        assert!(config
            .validate()
            .unwrap_err()
            .contains("network_outage_grace_seconds"));
    }

    #[test]
    fn rejects_zero_idle_grace() {
        let mut config = Config::default();
        config.session.idle_grace_seconds = 0;
        assert!(config
            .validate()
            .unwrap_err()
            .contains("idle_grace_seconds"));
    }

    #[test]
    fn rejects_unknown_configuration_fields() {
        let error = toml::from_str::<Config>("version = 1\nunknown_setting = true\n").unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}
