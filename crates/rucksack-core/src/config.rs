use crate::files::{atomic_write_toml, read_toml};
use crate::paths::AppPaths;
use crate::protocol::MAX_HELPER_TTL_SECONDS;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const CONFIG_VERSION: u32 = 1;

/// Configuration written by older releases must keep loading.
///
/// Every struct here omits `deny_unknown_fields` on purpose: a config file left behind by a
/// version that still had `focus`, `idle_grace_seconds`, or `require_verified_ssid` loads and
/// ignores them instead of failing the command the user is trying to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub hotspot: HotspotConfig,
    pub session: SessionConfig,
    pub safety: SafetyConfig,
    pub adapters: AdaptersConfig,
}

/// Which coding agents rucksack is allowed to touch.
///
/// All three default to on, because rucksack cannot know which agents a Mac has until it looks and
/// the look is cheap. Setting one to `false` is how someone says "I do not use that one", and it
/// must be taken literally: nothing for that agent runs, not even asking its CLI whether it is
/// there. Nothing here is a safety setting — no combination of these flags can affect the lease.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AdaptersConfig {
    pub codex: bool,
    pub claude: bool,
    pub cursor: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotspotConfig {
    /// The network `pack` joins. Learned from the first successful pack.
    pub ssid: Option<String>,
    pub require_iphone_usb: bool,
    pub probe_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub duration_minutes: u64,
    pub heartbeat_seconds: u64,
    pub helper_ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    pub warn_battery_percent: u8,
    /// The watcher restores normal sleep here, so the Mac never runs itself flat.
    pub sleep_battery_percent: u8,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            hotspot: HotspotConfig::default(),
            session: SessionConfig::default(),
            safety: SafetyConfig::default(),
            adapters: AdaptersConfig::default(),
        }
    }
}

impl Default for AdaptersConfig {
    fn default() -> Self {
        Self {
            codex: true,
            claude: true,
            cursor: true,
        }
    }
}

impl Default for HotspotConfig {
    fn default() -> Self {
        Self {
            ssid: None,
            require_iphone_usb: false,
            probe_timeout_seconds: 6,
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            duration_minutes: 24 * 60,
            heartbeat_seconds: 30,
            helper_ttl_seconds: 90,
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            warn_battery_percent: 20,
            sleep_battery_percent: 15,
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
                "Unsupported config version {}; expected {CONFIG_VERSION}",
                self.version
            ));
        }
        if self.safety.warn_battery_percent > 100 || self.safety.sleep_battery_percent > 100 {
            return Err("Battery thresholds must be between 0 and 100".to_owned());
        }
        if self.safety.sleep_battery_percent >= self.safety.warn_battery_percent {
            return Err("sleep_battery_percent must be lower than warn_battery_percent".to_owned());
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
                "hotspot.ssid must be a non-empty network name without control characters"
                    .to_owned(),
            );
        }
        if self.hotspot.require_iphone_usb && self.hotspot.ssid.is_some() {
            return Err("require_iphone_usb cannot be combined with a Wi-Fi hotspot".to_owned());
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
        // A fresh install has every adapter on, so a Mac with one agent is not asked to configure
        // anything before `pack` works.
        assert!(config.adapters.codex);
        assert!(config.adapters.claude);
        assert!(config.adapters.cursor);
    }

    /// The setting exists to be obeyed, so it has to survive the round trip a user relies on.
    #[test]
    fn an_adapter_switched_off_stays_off() {
        let written = "version = 1\n\n[adapters]\ncodex = false\n";

        let parsed = toml::from_str::<Config>(written).unwrap();

        assert!(parsed.validate().is_ok());
        assert!(!parsed.adapters.codex);
        // Only what was named is changed.
        assert!(parsed.adapters.claude);
        assert!(parsed.adapters.cursor);

        let round_tripped = toml::from_str::<Config>(&toml::to_string(&parsed).unwrap()).unwrap();
        assert!(!round_tripped.adapters.codex);
    }

    /// A config file from any earlier release must not break the command the user just typed.
    #[test]
    fn config_written_by_older_releases_still_loads() {
        let legacy = "version = 1\n\
             default_agent = \"codex\"\n\
             \n\
             [hotspot]\n\
             ssid = \"Noah\"\n\
             require_verified_ssid = true\n\
             probe_timeout_seconds = 6\n\
             \n\
             [session]\n\
             duration_minutes = 1440\n\
             focus = \"continue\"\n\
             heartbeat_seconds = 30\n\
             helper_ttl_seconds = 90\n\
             idle_grace_seconds = 900\n\
             network_outage_grace_seconds = 300\n\
             stop_owned_remote_on_unpack = false\n\
             \n\
             [safety]\n\
             minimum_start_battery_percent = 35\n\
             warn_battery_percent = 20\n\
             sleep_battery_percent = 15\n\
             \n\
             [adapters]\n\
             codex = true\n";

        let parsed = toml::from_str::<Config>(legacy).unwrap();

        assert!(parsed.validate().is_ok());
        assert_eq!(parsed.hotspot.ssid.as_deref(), Some("Noah"));
        assert_eq!(parsed.session.duration_minutes, 1440);
        assert!(parsed.adapters.codex);
    }

    #[test]
    fn rejects_thresholds_that_would_release_before_warning() {
        let mut config = Config::default();
        config.safety.sleep_battery_percent = config.safety.warn_battery_percent;
        assert!(config
            .validate()
            .unwrap_err()
            .contains("sleep_battery_percent"));
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
    fn rejects_hotspots_that_are_empty_or_carry_control_characters() {
        let mut config = Config::default();
        config.hotspot.ssid = Some("  ".to_owned());
        assert!(config.validate().unwrap_err().contains("hotspot.ssid"));

        config.hotspot.ssid = Some("Noah\nforged".to_owned());
        assert!(config.validate().unwrap_err().contains("hotspot.ssid"));
    }

    #[test]
    fn rejects_conflicting_wifi_and_usb_requirements() {
        let mut config = Config::default();
        config.hotspot.require_iphone_usb = true;
        config.hotspot.ssid = Some("Noah".to_owned());
        assert!(config
            .validate()
            .unwrap_err()
            .contains("require_iphone_usb"));
    }
}
