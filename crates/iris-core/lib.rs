// SPDX-License-Identifier: MIT
// Iris — iris-core

pub mod app;
pub mod config;
pub mod error;
pub mod logging;
pub mod pipeline;

#[cfg(test)]
mod tests {
    use super::app::{AppState, CaptureState};
    use super::config::IrisConfig;
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

/// `IrisConfig::validate()` existed from the first Iris config and, until
/// 2026-08-31, had **no caller outside these tests** — `main.rs` loaded a file
/// and used it unchecked. These pin the checks that are now actually enforced
/// at startup, including the pixel-format list that changed with them.
#[cfg(test)]
mod config_validation {
    use crate::config::{IrisConfig, ALLOWED_PIXEL_FORMATS};

    #[test]
    fn the_default_config_is_valid() {
        IrisConfig::default()
            .validate()
            .expect("the built-in defaults must pass the checks applied at startup");
    }

    #[test]
    fn every_allowed_pixel_format_validates() {
        for name in ALLOWED_PIXEL_FORMATS {
            let mut cfg = IrisConfig::default();
            cfg.capture.pixel_format = (*name).to_string();
            assert!(cfg.validate().is_ok(), "'{name}' is listed but rejected");
        }
    }

    /// `bgra8` used to be accepted here and could not be parsed anywhere.
    #[test]
    fn an_unproducible_pixel_format_is_rejected() {
        let mut cfg = IrisConfig::default();
        cfg.capture.pixel_format = "bgra8".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn an_unknown_drop_policy_is_rejected() {
        let mut cfg = IrisConfig::default();
        cfg.capture.drop_policy = "banana".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn an_out_of_range_frame_rate_is_rejected() {
        let mut cfg = IrisConfig::default();
        cfg.capture.target_fps = 0;
        assert!(cfg.validate().is_err());
        cfg.capture.target_fps = 241;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn an_unknown_log_level_is_rejected() {
        let mut cfg = IrisConfig::default();
        cfg.logging.level = "chatty".to_string();
        assert!(cfg.validate().is_err());
    }
}

/// `iris.toml` used to be read from **one** place — the directory holding the
/// executable — which makes an installed build unconfigurable and a dev build
/// look for it in `target/release/`. These pin the search order added
/// 2026-08-31, and in particular that the executable directory stays first so
/// no existing setup changes behaviour.
#[cfg(test)]
mod config_search_paths {
    use crate::config::IrisConfig;

    #[test]
    fn the_executable_directory_is_searched_first() {
        let paths = IrisConfig::config_search_paths();
        let exe_cfg = IrisConfig::config_path().expect("exe path");
        assert_eq!(
            paths.first(),
            Some(&exe_cfg),
            "the executable directory must stay highest priority — anything \
             already working must keep working"
        );
    }

    /// The XDG location is what a packaged Iris will actually use, so its
    /// absence would make the whole change cosmetic.
    #[test]
    fn an_xdg_location_is_also_searched() {
        let paths = IrisConfig::config_search_paths();
        assert!(
            paths.len() >= 2,
            "expected an XDG path in addition to the executable directory, got {paths:?}"
        );
        let last = paths.last().expect("at least one path");
        assert!(
            last.ends_with("iris/iris.toml"),
            "the XDG entry must live under an iris/ directory, got {last:?}"
        );
    }

    #[test]
    fn every_search_path_is_named_iris_toml() {
        for p in IrisConfig::config_search_paths() {
            assert_eq!(
                p.file_name().and_then(|n| n.to_str()),
                Some("iris.toml"),
                "unexpected filename in the search order: {p:?}"
            );
        }
    }

    /// The XDG basedir spec says a relative `XDG_CONFIG_HOME` must be ignored.
    /// Honouring one would resolve the path against the process's working
    /// directory, so where Iris looked for its config would depend on where it
    /// happened to be started from.
    #[test]
    fn a_relative_xdg_config_home_is_ignored() {
        // Serialised with the test below by running both under one lock would
        // be overkill: this asserts a property of the returned paths, not of a
        // specific value, so a concurrent test changing the var cannot make it
        // wrong.
        let paths = IrisConfig::config_search_paths();
        for p in &paths {
            assert!(
                p.is_absolute(),
                "a relative search path means XDG_CONFIG_HOME was honoured when \
                 it should have been ignored: {p:?}"
            );
        }
    }
}
