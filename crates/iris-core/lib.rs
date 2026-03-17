// SPDX-License-Identifier: MIT
// Iris — iris-core

pub mod app;
pub mod config;
pub mod error;
pub mod logging;

#[cfg(test)]
mod tests {
    use super::app::{AppState, CaptureState};
    use super::config::IrisConfig;
    use serde::{Deserialize, Serialize};
    use toml;

    #[test]
    fn test_default_config() {
        let cfg = IrisConfig::default();
        cfg.validate().expect("default config should validate");
    }

    #[test]
    fn test_config_roundtrip() {
        let cfg = IrisConfig::default();
        let s = toml::to_string(&cfg).expect("serialize");
        let parsed: IrisConfig = toml::from_str(&s).expect("deserialize");
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn test_config_validation_valid() {
        let cfg = IrisConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_fps() {
        let mut cfg = IrisConfig::default();
        cfg.capture.target_fps = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_resolution() {
        let mut cfg = IrisConfig::default();
        cfg.capture.width = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_pixel_format() {
        let mut cfg = IrisConfig::default();
        cfg.capture.pixel_format = "rgb565".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_drop_policy() {
        let mut cfg = IrisConfig::default();
        cfg.capture.drop_policy = "random".to_string();
        assert!(cfg.validate().is_err());
    }

    #[tokio::test]
    async fn test_app_state_capture_state() {
        let app = AppState::new();
        let mut rx = app.subscribe_capture_state();
        app.set_capture_state(CaptureState::Initializing);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), CaptureState::Initializing);
    }

    #[tokio::test]
    async fn test_app_state_fps_update() {
        let app = AppState::new();
        let mut rx = app.subscribe_current_fps();
        app.set_current_fps(29.5);
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), 29.5);
    }

    #[tokio::test]
    async fn test_app_state_device_name() {
        let app = AppState::new();
        let mut rx = app.subscribe_device_name();
        app.set_device_name("TestCam".to_string());
        rx.changed().await.unwrap();
        assert_eq!(&**rx.borrow(), "TestCam");
    }
}
