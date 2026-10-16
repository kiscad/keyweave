//! Rust settings GUI: offline editor only; runtime control is a separate owner.
pub mod control;
pub mod devices;
pub mod devices_view;
pub mod files;
#[cfg(target_os = "linux")]
pub mod inventory;
pub mod model;
pub mod rules;
pub mod rules_view;
pub mod view;
pub mod visual;
pub mod visual_view;
