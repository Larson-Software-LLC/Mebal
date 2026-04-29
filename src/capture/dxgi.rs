// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! DXGI Desktop Duplication capture
//!
//! Uses the Windows Desktop Duplication API to capture the desktop compositor
//! output directly from the GPU, producing BGRA textures with minimal CPU overhead.

use anyhow::{Context, Result};
use tracing::{debug, warn};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::{HRESULT, Interface};

const DXGI_ERROR_WAIT_TIMEOUT: HRESULT = HRESULT(0x887A0027u32 as i32);
const DXGI_ERROR_ACCESS_LOST: HRESULT = HRESULT(0x887A0026u32 as i32);

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
}

impl DxgiCapture {
    pub fn new(output_index: u32) -> Result<Self> {
        // Create D3D11 device
        let mut device = None;
        let mut context = None;
        unsafe {
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
            .context("D3D11CreateDevice failed")?;
        }
        let device = device.context("D3D11 device was null")?;
        let context = context.context("D3D11 device context was null")?;

        // Get DXGI adapter and output
        let dxgi_device: IDXGIDevice = device.cast().context("Failed to cast to IDXGIDevice")?;
        let adapter = unsafe { dxgi_device.GetAdapter().context("GetAdapter failed")? };
        let output = unsafe {
            adapter.EnumOutputs(output_index).context(format!(
                "EnumOutputs({}) failed — check monitor index",
                output_index
            ))?
        };
        // Get output dimensions
        let desc = unsafe { output.GetDesc().context("GetDesc failed")? };

        let output1: IDXGIOutput1 = output
            .cast()
            .context("Failed to cast to IDXGIOutput1 — DXGI 1.2+ required")?;
        let rect = desc.DesktopCoordinates;
        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;
        debug!("DXGI output {}: {}x{}", output_index, width, height);

        // Duplicate output
        let duplication = unsafe {
            output1
                .DuplicateOutput(&device)
                .context("DuplicateOutput failed — is another app using Desktop Duplication?")?
        };

        // Create staging texture for CPU readback
        let staging = Self::create_staging_texture(&device, width, height)?;

        Ok(Self {
            device,
            context,
            duplication,
            staging,
            width,
            height,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Acquire a frame and copy BGRA pixels into `dst`.
    ///
    /// Returns `Ok(true)` if a new frame was captured, `Ok(false)` on timeout
    /// (desktop unchanged). Returns `Err` on access lost or other failures.
    pub fn acquire_frame_into(
        &mut self,
        dst: &mut [u8],
        dst_stride: usize,
        timeout_ms: u32,
    ) -> Result<bool> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        let hr = unsafe {
            self.duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
        };

        match hr {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                anyhow::bail!("DXGI access lost — desktop duplication needs reconnect");
            }
            Err(e) => return Err(e.into()),
        }

        let resource = resource.context("AcquireNextFrame returned null resource")?;
        let texture: ID3D11Texture2D = resource.cast().context("Failed to cast to Texture2D")?;

        // GPU copy to staging texture
        unsafe {
            self.context.CopyResource(&self.staging, &texture);
        }

        // Release DXGI frame immediately (before blocking on Map)
        unsafe {
            self.duplication.ReleaseFrame().ok();
        }

        // Map staging texture for CPU read
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("Failed to map staging texture")?;
        }

        // Row-by-row copy (handles stride mismatch between GPU and destination)
        let src_stride = mapped.RowPitch as usize;
        let row_bytes = (self.width * 4) as usize;
        let src_ptr = mapped.pData as *const u8;
        for y in 0..self.height as usize {
            let src_row =
                unsafe { std::slice::from_raw_parts(src_ptr.add(y * src_stride), row_bytes) };
            let dst_offset = y * dst_stride;
            dst[dst_offset..dst_offset + row_bytes].copy_from_slice(src_row);
        }

        unsafe {
            self.context.Unmap(&self.staging, 0);
        }

        Ok(true)
    }

    /// Reconnect after access lost (e.g. resolution change, lock screen).
    pub fn reconnect(&mut self, output_index: u32) -> Result<()> {
        let dxgi_device: IDXGIDevice = self
            .device
            .cast()
            .context("Failed to cast to IDXGIDevice")?;
        let adapter = unsafe { dxgi_device.GetAdapter().context("GetAdapter failed")? };
        let output = unsafe {
            adapter.EnumOutputs(output_index).context(format!(
                "EnumOutputs({}) failed during reconnect",
                output_index
            ))?
        };
        let desc = unsafe { output.GetDesc().context("GetDesc failed")? };

        let output1: IDXGIOutput1 = output.cast().context("Failed to cast to IDXGIOutput1")?;
        let rect = desc.DesktopCoordinates;
        let new_width = (rect.right - rect.left) as u32;
        let new_height = (rect.bottom - rect.top) as u32;

        self.duplication = unsafe {
            output1
                .DuplicateOutput(&self.device)
                .context("DuplicateOutput failed during reconnect")?
        };

        if new_width != self.width || new_height != self.height {
            warn!(
                "Resolution changed: {}x{} -> {}x{}",
                self.width, self.height, new_width, new_height
            );
            self.staging = Self::create_staging_texture(&self.device, new_width, new_height)?;
            self.width = new_width;
            self.height = new_height;
        }

        debug!("DXGI reconnected: {}x{}", self.width, self.height);
        Ok(())
    }

    fn create_staging_texture(
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: Default::default(),
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: Default::default(),
        };

        let mut texture = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .context("Failed to create staging texture")?;
        }
        texture.context("Staging texture was null")
    }
}
