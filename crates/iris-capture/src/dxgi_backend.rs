#![allow(non_snake_case)]

use async_trait::async_trait;
use tracing::info;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::backend::CaptureBackend;
use crate::backend::CaptureConfig;
use crate::frame::CaptureFrame;
use iris_core::error::{IrisError, IrisResult};
// Note: dxgi backend maps HAL pixel formats into `iris_core::PixelFormat` when
// creating `CaptureFrame` instances.

pub struct DxgiCaptureBackend {
    device: Option<ID3D11Device>,
    device_context: Option<ID3D11DeviceContext>,
    duplication: Option<IDXGIOutputDuplication>,
    staging_texture: Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
    capturing: bool,
    sequence: u64,
    _config: CaptureConfig,
}

unsafe impl Send for DxgiCaptureBackend {}
unsafe impl Sync for DxgiCaptureBackend {}

impl DxgiCaptureBackend {
    pub fn new(config: CaptureConfig) -> Self {
        Self {
            device: None,
            device_context: None,
            duplication: None,
            staging_texture: None,
            width: config.width,
            height: config.height,
            capturing: false,
            sequence: 0,
            _config: config,
        }
    }
}

#[async_trait]
impl CaptureBackend for DxgiCaptureBackend {
    async fn start(&mut self) -> IrisResult<()> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()
                .map_err(|e| IrisError::Capture(format!("CreateDXGIFactory1: {e:?}")))?;

            let adapter: IDXGIAdapter1 = factory
                .EnumAdapters1(0)
                .map_err(|e| IrisError::Capture(format!("EnumAdapters1: {e:?}")))?;

            let output: IDXGIOutput = adapter
                .EnumOutputs(0)
                .map_err(|e| IrisError::Capture(format!("EnumOutputs: {e:?}")))?;

            // Use configured capture size; desktop query omitted to avoid
            // binding-specific method resolution issues in these bindings.

            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;

            // Use None for adapter/software/feature-levels to match windows-rs signatures
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| IrisError::Capture(format!("D3D11CreateDevice: {e:?}")))?;

            let device = device.ok_or(IrisError::Capture("D3D11 device is None".into()))?;
            let context = context.ok_or(IrisError::Capture("D3D11 context is None".into()))?;

            let output1: IDXGIOutput1 = output
                .cast()
                .map_err(|e| IrisError::Capture(format!("Cast to IDXGIOutput1: {e:?}")))?;
            let duplication = output1
                .DuplicateOutput(&device)
                .map_err(|e| IrisError::Capture(format!("DuplicateOutput: {e:?}")))?;

            let tex_desc = D3D11_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };

            let mut staging_opt: Option<ID3D11Texture2D> = None;
            device
                .CreateTexture2D(&tex_desc, None, Some(&mut staging_opt))
                .map_err(|e| IrisError::Capture(format!("CreateTexture2D: {e:?}")))?;
            let staging =
                staging_opt.ok_or(IrisError::Capture("CreateTexture2D returned None".into()))?;

            self.device = Some(device);
            self.device_context = Some(context);
            self.duplication = Some(duplication);
            self.staging_texture = Some(staging);
            self.capturing = true;

            info!("DXGI capture started: {}x{}", self.width, self.height);
            Ok(())
        }
    }

    async fn stop(&mut self) -> IrisResult<()> {
        self.capturing = false;
        // Release COM resources
        self.duplication = None;
        self.staging_texture = None;
        self.device_context = None;
        self.device = None;
        info!("DXGI capture stopped");
        Ok(())
    }

    async fn next_frame(&mut self) -> IrisResult<CaptureFrame> {
        if !self.capturing {
            return Err(IrisError::Capture("not capturing".into()));
        }

        unsafe {
            let duplication = self
                .duplication
                .as_ref()
                .ok_or(IrisError::Capture("no duplication".into()))?;
            let context = self
                .device_context
                .as_ref()
                .ok_or(IrisError::Capture("no context".into()))?;
            let staging = self
                .staging_texture
                .as_ref()
                .ok_or(IrisError::Capture("no staging texture".into()))?;

            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut desktop_resource: Option<IDXGIResource> = None;

            duplication
                .AcquireNextFrame(500, &mut frame_info, &mut desktop_resource)
                .map_err(|e| IrisError::Capture(format!("AcquireNextFrame: {e:?}")))?;

            let desktop_resource =
                desktop_resource.ok_or(IrisError::Capture("Desktop resource is None".into()))?;
            let desktop_texture: ID3D11Texture2D = desktop_resource
                .cast()
                .map_err(|e| IrisError::Capture(format!("Cast to Texture2D: {e:?}")))?;

            context.CopyResource(staging, &desktop_texture);
            duplication
                .ReleaseFrame()
                .map_err(|e| IrisError::Capture(format!("ReleaseFrame: {e:?}")))?;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| IrisError::Capture(format!("Map: {e:?}")))?;

            let src_stride = mapped.RowPitch as usize;
            // We'll produce BGR24 output by dropping alpha from BGRA8
            let dst_stride = (self.width * 3) as usize;
            let mut data = Vec::with_capacity(dst_stride * self.height as usize);

            let src_ptr = mapped.pData as *const u8;
            for row in 0..self.height {
                let base = (row as usize) * src_stride;
                for col in 0..(self.width as usize) {
                    let off = base + col * 4;
                    let b = *src_ptr.add(off);
                    let g = *src_ptr.add(off + 1);
                    let r = *src_ptr.add(off + 2);
                    data.push(r);
                    data.push(g);
                    data.push(b);
                }
            }

            context.Unmap(staging, 0);

            self.sequence = self.sequence.wrapping_add(1);
            let frame = CaptureFrame {
                sequence: self.sequence,
                width: self.width,
                height: self.height,
                format: iris_core::PixelFormat::Bgr24,
                data,
                timestamp_us: CaptureFrame::now_us(),
                is_cropped: false,
            };

            Ok(frame)
        }
    }

    fn is_capturing(&self) -> bool {
        self.capturing
    }
}
