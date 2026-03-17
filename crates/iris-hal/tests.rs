#[cfg(test)]
mod tests {
    use crate::backend::{MockUvcBackend, UvcBackend};
    use crate::device::DeviceId;

    #[tokio::test]
    async fn enumerate_returns_mock_device() {
        let backend = MockUvcBackend::new();
        let devs = backend.enumerate_devices().await.unwrap();
        assert!(!devs.is_empty());
        assert_eq!(devs[0].id.0, "mock-0");
    }

    #[tokio::test]
    async fn probe_capabilities_succeeds() {
        let backend = MockUvcBackend::new();
        let caps = backend
            .probe_capabilities(&DeviceId("mock-0".into()))
            .await
            .unwrap();
        assert!(!caps.formats.is_empty());
    }

    #[tokio::test]
    async fn open_close_device_flow() {
        let backend = MockUvcBackend::new();
        let id = DeviceId("mock-0".into());
        backend.open_device(&id).await.unwrap();
        let frame = backend.read_frame(&id).await.unwrap();
        assert!(!frame.is_empty());
        backend.close_device(&id).await.unwrap();
    }

    #[tokio::test]
    async fn control_get_set() {
        let backend = MockUvcBackend::new();
        let id = DeviceId("mock-0".into());
        let list = backend.list_controls(&id).await.unwrap();
        assert!(!list.is_empty());
        let val = backend.get_control(&id, list[0].id).await.unwrap();
        assert!(val >= list[0].min && val <= list[0].max);
        backend.set_control(&id, list[0].id, 200).await.unwrap();
        let val2 = backend.get_control(&id, list[0].id).await.unwrap();
        assert_eq!(val2, 200);
    }

    #[tokio::test]
    async fn read_frame_requires_open() {
        let backend = MockUvcBackend::new();
        let id = DeviceId("mock-0".into());
        let res = backend.read_frame(&id).await;
        assert!(res.is_err());
    }

    /// Hardware-gated WMF test — only runs when IRIS_USE_HW=1.
    /// Requires a real webcam plugged in.
    #[cfg(windows)]
    #[tokio::test]
    async fn wmf_enumerate_real_devices() {
        if std::env::var("IRIS_USE_HW").as_deref() != Ok("1") {
            eprintln!("skipping wmf_enumerate_real_devices (set IRIS_USE_HW=1)");
            return;
        }
        let backend = crate::backend::new_wmf_backend().expect("WmfBackend::new() failed");
        let devs = backend
            .enumerate_devices()
            .await
            .expect("enumerate_devices failed");
        eprintln!("WMF devices found: {}", devs.len());
        for d in &devs {
            eprintln!("  {} — {}", d.id, d.name);
        }
        assert!(!devs.is_empty(), "No video capture devices found");
    }
}
