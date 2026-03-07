//! `AirPods` D-Bus Service for KDE Plasma
//!
//! This service provides a D-Bus interface for managing `AirPods` devices
//! in KDE Plasma, including battery monitoring, noise control, and
//! feature management.

use std::{sync::Arc, time::Duration};

use crossbeam::queue::SegQueue;
use log::{info, warn};
use tokio::{signal, sync::Notify, task::JoinHandle, time};
use zbus::{Connection, connection, object_server::InterfaceRef};

use bluetooth::manager::BluetoothManager;
use dbus::AirPodsService;
use event::{AirPodsEvent, EventBus};

mod airpods;
mod battery_provider;
mod battery_study;
mod bluetooth;
mod config;
mod dbus;
mod error;
mod event;
mod media_control;
mod ringbuf;

use crate::{
   airpods::{
      device::AirPods,
      protocol::{NoiseControlMode, StemPressType},
   },
   config::GestureAction,
   dbus::AirPodsServiceSignals,
   error::Result,
};

#[tokio::main]
async fn main() -> Result<()> {
   // Parse command line arguments
   let args: Vec<String> = std::env::args().collect();
   if args.len() > 1 {
      match args[1].as_str() {
         "--version" | "-v" => {
            println!("kairpodsd {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
         },
         "--help" | "-h" => {
            println!("Usage: {} [OPTIONS]", args[0]);
            println!();
            println!("Options:");
            println!("  -v, --version    Print version information and exit");
            println!("  -h, --help       Print this help message and exit");
            return Ok(());
         },
         arg => {
            eprintln!("Unknown argument: {arg}");
            eprintln!("Try '{} --help' for more information.", args[0]);
            std::process::exit(1);
         },
      }
   }

   let (config, config_err) = match config::Config::load() {
      Ok(config) => (config, None),
      Err(e) => (config::Config::default(), Some(e)),
   };

   let default_filter = config.log_filter.as_deref().unwrap_or("info");
   env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_filter))
      .init();
   info!("Starting kAirPods D-Bus service...");

   if let Some(err) = config_err {
      warn!("Failed to load configuration: {err:?}");
   } else {
      info!(
         "Loaded configuration with {} known devices",
         config.known_devices.len()
      );
   }

   // Create event channel
   let event_bus = EventProcessor::new(config.gestures.clone());

   // Initialize battery study database
   let battery_study = match battery_study::BatteryStudy::open() {
      Ok(study) => {
         info!("Battery study database initialized");
         Some(study)
      },
      Err(e) => {
         warn!("Failed to initialize battery study database: {e}");
         None
      },
   };

   // Create Bluetooth manager with event sender and config
   let bluetooth_manager = BluetoothManager::new(event_bus.clone(), config, battery_study).await?;

   // Create D-Bus service
   let service = AirPodsService::new(bluetooth_manager);

   // Build D-Bus connection
   let connection = connection::Builder::session()?
      .name("org.kairpods")?
      .serve_at("/org/kairpods/manager", service)?
      .build()
      .await?;

   info!("kAirPods D-Bus service started at org.kairpods");

   // Initialize BlueZ battery provider for UPower integration
   let battery_provider = battery_provider::BatteryProvider::new().await;
   if battery_provider.is_none() {
      warn!("BlueZ battery provider unavailable, UPower integration disabled");
   }

   // Start event processor
   let shutdown = Arc::new(Notify::new());
   let dispatcher = event_bus
      .spawn_dispatcher(connection, battery_provider, shutdown.clone())
      .await?;

   // Wait for shutdown signal
   signal::ctrl_c().await?;
   info!("Shutting down kAirPods service...");
   shutdown.notify_one();
   let _ = dispatcher.await;

   Ok(())
}

struct EventProcessor {
   queue: SegQueue<(AirPods, AirPodsEvent)>,
   notifier: Notify,
   gesture_config: config::GestureConfig,
}

impl EventProcessor {
   fn new(gesture_config: config::GestureConfig) -> Arc<Self> {
      Arc::new(Self {
         queue: SegQueue::new(),
         notifier: Notify::new(),
         gesture_config,
      })
   }
}

impl EventProcessor {
   async fn recv(self: &Arc<Self>) -> Option<(AirPods, AirPodsEvent)> {
      loop {
         if let Some(event) = self.queue.pop() {
            return Some(event);
         }
         let notify = self.notifier.notified();
         if let Some(event) = self.queue.pop() {
            return Some(event);
         }
         if Arc::strong_count(self) == 1 {
            return None;
         }
         let _ = time::timeout(Duration::from_secs(1), notify).await;
      }
   }

   async fn dispatch(
      &self,
      iface: &InterfaceRef<AirPodsService>,
      battery_provider: &mut Option<battery_provider::BatteryProvider>,
      (device, event): (AirPods, AirPodsEvent),
   ) -> Result<()> {
      let addr_str = device.address_str();
      match event {
         AirPodsEvent::DeviceConnected => {
            iface.device_connected(addr_str).await?;
            // Emit property changes
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;
            iface
               .get_mut()
               .await
               .connected_count_changed(iface.signal_emitter())
               .await?;
         },
         AirPodsEvent::DeviceDisconnected => {
            iface.device_disconnected(addr_str).await?;
            // Remove from BlueZ battery provider
            if let Some(bp) = battery_provider.as_mut() {
               bp.remove(device.address()).await;
            }
            // Emit property changes
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;
            iface
               .get_mut()
               .await
               .connected_count_changed(iface.signal_emitter())
               .await?;
         },
         AirPodsEvent::BatteryUpdated(battery) => {
            iface
               .battery_updated(addr_str, &battery.to_json().to_string())
               .await?;
            // Update BlueZ battery provider for UPower integration
            if let Some(bp) = battery_provider.as_mut() {
               let percentage = if battery.headphone.is_available() {
                  battery.headphone.level
               } else {
                  let mut level = u8::MAX;
                  if battery.left.is_available() {
                     level = level.min(battery.left.level);
                  }
                  if battery.right.is_available() {
                     level = level.min(battery.right.level);
                  }
                  if level == u8::MAX { 0 } else { level }
               };
               bp.update(device.address(), percentage).await;
            }
            // Emit property change for devices (battery state changed)
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;
         },
         AirPodsEvent::NoiseControlChanged(mode) => {
            iface.noise_control_changed(addr_str, mode.to_str()).await?;
            // Emit property change for devices (noise control state changed)
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;
         },
         AirPodsEvent::EarDetectionChanged(ear_detection) => {
            iface
               .ear_detection_changed(addr_str, &ear_detection.to_json().to_string())
               .await?;
            // Emit property change for devices (ear detection state changed)
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;

            // Handle play/pause based on ear detection
            // Pause when at least one earbud is removed, play only when both are in
            let both_in_ear = ear_detection.is_left_in_ear() && ear_detection.is_right_in_ear();
            if both_in_ear {
               // Both AirPods are in ear - send play command
               media_control::send_play().await;
            } else {
               // At least one AirPod is out of ear - send pause command
               media_control::send_pause().await;
            }
         },
         AirPodsEvent::StemPressed(stem_event) => {
            iface
               .stem_pressed(addr_str, &stem_event.to_json().to_string())
               .await?;

            let action = match stem_event.press_type {
               StemPressType::Single => &self.gesture_config.single_press,
               StemPressType::Double => &self.gesture_config.double_press,
               StemPressType::Triple => &self.gesture_config.triple_press,
               StemPressType::Long => &self.gesture_config.long_press,
            };

            match action {
               GestureAction::PlayPause => media_control::send_play_pause().await,
               GestureAction::Next => media_control::send_next().await,
               GestureAction::Previous => media_control::send_previous().await,
               GestureAction::CycleNoiseMode => {
                  if let Some(current_mode) = device.noise_mode() {
                     let next_mode = device
                        .prev_noise_mode()
                        .filter(|prev| *prev != current_mode)
                        .unwrap_or(if current_mode == NoiseControlMode::Active {
                           NoiseControlMode::Transparency
                        } else {
                           NoiseControlMode::Active
                        });
                     if let Err(e) = device.set_noise_control(next_mode).await {
                        warn!("Failed to toggle noise mode: {e}");
                     } else {
                        // Notify UI of the noise mode change
                        iface
                           .noise_control_changed(addr_str, next_mode.to_str())
                           .await?;
                        iface
                           .get_mut()
                           .await
                           .devices_changed(iface.signal_emitter())
                           .await?;
                     }
                  }
               },
               GestureAction::None => {},
            }
         },
         AirPodsEvent::DeviceNameChanged(name) => {
            iface.device_name_changed(addr_str, &name).await?;
            // Emit property change for devices (name changed)
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;
         },
         AirPodsEvent::DeviceError => {
            iface.device_error(addr_str).await?;
            // Emit property change for devices (error state might affect device info)
            iface
               .get_mut()
               .await
               .devices_changed(iface.signal_emitter())
               .await?;
         },
      }
      Ok(())
   }

   async fn spawn_dispatcher(
      self: Arc<Self>,
      connection: Connection,
      mut battery_provider: Option<battery_provider::BatteryProvider>,
      shutdown: Arc<Notify>,
   ) -> Result<JoinHandle<()>> {
      let iface = connection
         .object_server()
         .interface::<_, AirPodsService>("/org/kairpods/manager")
         .await?;
      let handle = tokio::spawn(async move {
         loop {
            tokio::select! {
               event = self.recv() => {
                  let Some(event) = event else { break };
                  if let Err(e) = self.dispatch(&iface, &mut battery_provider, event).await {
                     warn!("Error dispatching event: {e}");
                  }
               }
               () = shutdown.notified() => break,
            }
         }
         if let Some(bp) = battery_provider.as_mut() {
            bp.shutdown().await;
         }
      });

      Ok(handle)
   }
}

impl EventBus for EventProcessor {
   fn emit(&self, device: &AirPods, event: AirPodsEvent) {
      self.queue.push((device.clone(), event));
      self.notifier.notify_waiters();
   }
}
