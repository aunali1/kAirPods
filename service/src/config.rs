//! Configuration management for the `AirPods` service.
//!
//! This module handles loading and saving configuration from disk,
//! including known devices and connection parameters.

use std::{env, fs, path::PathBuf};

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

use crate::error::{AirPodsError, Result};

/// Main configuration structure for the service.
#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
   #[serde(default)]
   pub known_devices: Vec<KnownDevice>,

   #[serde(default = "default_poll_interval")]
   pub poll_interval: u64,

   #[serde(default = "default_retry_count")]
   pub connection_retry_count: u32,

   #[serde(default = "default_reconnect_delay")]
   pub reconnect_delay_sec: u64,

   #[serde(default = "default_notification_retries")]
   pub notification_retries: u32,

   #[serde(default)]
   pub log_filter: Option<SmolStr>,

   #[serde(default)]
   pub gestures: GestureConfig,
}

/// Represents a known `AirPods` device.
#[derive(Serialize, Deserialize, Clone)]
pub struct KnownDevice {
   pub address: String,
   pub name: String,
}

const fn default_poll_interval() -> u64 {
   30
}

const fn default_retry_count() -> u32 {
   10
}

const fn default_notification_retries() -> u32 {
   3
}

const fn default_reconnect_delay() -> u64 {
   10
}

/// Action to perform for a stem press gesture.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GestureAction {
   PlayPause,
   Next,
   Previous,
   CycleNoiseMode,
   None,
}

/// Configuration for stem press gesture handling.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GestureConfig {
   /// Whether to intercept stem presses via AAP (vs letting AVRCP handle them).
   #[serde(default = "default_gesture_enabled")]
   pub enabled: bool,

   #[serde(default = "default_single_press_action")]
   pub single_press: GestureAction,

   #[serde(default = "default_double_press_action")]
   pub double_press: GestureAction,

   #[serde(default = "default_triple_press_action")]
   pub triple_press: GestureAction,

   #[serde(default = "default_long_press_action")]
   pub long_press: GestureAction,
}

const fn default_gesture_enabled() -> bool {
   true
}

fn default_single_press_action() -> GestureAction {
   GestureAction::PlayPause
}

fn default_double_press_action() -> GestureAction {
   GestureAction::Next
}

fn default_triple_press_action() -> GestureAction {
   GestureAction::Previous
}

fn default_long_press_action() -> GestureAction {
   GestureAction::CycleNoiseMode
}

impl Default for GestureConfig {
   fn default() -> Self {
      Self {
         enabled: default_gesture_enabled(),
         single_press: default_single_press_action(),
         double_press: default_double_press_action(),
         triple_press: default_triple_press_action(),
         long_press: default_long_press_action(),
      }
   }
}

impl Default for Config {
   fn default() -> Self {
      Self {
         known_devices: vec![],
         poll_interval: default_poll_interval(),
         connection_retry_count: default_retry_count(),
         reconnect_delay_sec: default_reconnect_delay(),
         notification_retries: default_notification_retries(),
         log_filter: None,
         gestures: GestureConfig::default(),
      }
   }
}

impl Config {
   /// Loads configuration from disk or creates default if not exists.
   pub fn load() -> Result<Self> {
      let config_path = Self::config_path()?;

      if config_path.exists() {
         let contents = fs::read_to_string(&config_path)?;
         Ok(toml::from_str(&contents)?)
      } else {
         // Create default config
         let config = Self::default();
         config.save()?;
         Ok(config)
      }
   }

   /// Saves the current configuration to disk.
   pub fn save(&self) -> Result<()> {
      let config_path = Self::config_path()?;

      // Ensure directory exists
      if let Some(parent) = config_path.parent() {
         fs::create_dir_all(parent)?;
      }

      let contents = toml::to_string_pretty(self)?;
      fs::write(&config_path, contents)?;

      Ok(())
   }

   fn config_path() -> Result<PathBuf> {
      // Check for override environment variable first
      if let Ok(path) = env::var("AIRPODS_CONFIG_PATH") {
         return Ok(PathBuf::from(path));
      }

      // Use dirs crate to get the proper config directory
      Ok(dirs::config_dir()
         .ok_or(AirPodsError::ConfigDirNotFound)?
         .join("kairpods")
         .join("config.toml"))
   }

   /// Checks if the given address is a known device and returns its name.
   pub fn is_known_device(&self, address: &str) -> Option<&str> {
      self
         .known_devices
         .iter()
         .find(|d| d.address == address)
         .map(|d| d.name.as_str())
   }
}
