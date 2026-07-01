// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! DXGI Desktop Duplication capture
//!
//! Uses the Windows Desktop Duplication API to capture the desktop compositor
//! output directly from the GPU as textures that stay in VRAM — copied GPU->GPU
//! into the encoder's hardware frame pool.
//!
//! On an SDR display the desktop is duplicated as 8-bit BGRA (sRGB) and passed
//! straight through. On an HDR display it is duplicated as FP16 scRGB (linear,
//! Rec.709 primaries, 1.0 == SDR reference white); the caller tone-maps it to SDR
//! before encoding (see `shader::Converter::hdr`).

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_OUTDUPL_FRAME_INFO, IDXGIDevice, IDXGIOutput1, IDXGIOutput5, IDXGIOutput6,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::{HRESULT, Interface};

const DXGI_ERROR_WAIT_TIMEOUT: HRESULT = HRESULT(0x887A0027u32 as i32);
const DXGI_ERROR_ACCESS_LOST: HRESULT = HRESULT(0x887A0026u32 as i32);

pub struct DxgiCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    width: u32,
    height: u32,
    /// True when the captured surface is FP16 scRGB (HDR display).
    hdr: bool,
    /// scRGB value of SDR reference white (nits / 80). Used to normalise HDR
    /// exposure before tone-mapping. 1.0 for SDR.
    sdr_white_scale: f32,
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

        // Multithread protection (NVENC/AMF touch the device from their own
        // threads) is enabled by FFmpeg's av_hwdevice_ctx_init in HwFrames.

        let dup = Self::duplicate(&device, output_index)?;
        info!(
            "DXGI output {}: {}x{} ({})",
            output_index,
            dup.width,
            dup.height,
            if dup.hdr {
                "HDR / FP16 scRGB"
            } else {
                "SDR / BGRA8"
            }
        );

        Ok(Self {
            device,
            context,
            duplication: dup.duplication,
            width: dup.width,
            height: dup.height,
            hdr: dup.hdr,
            sdr_white_scale: dup.sdr_white_scale,
        })
    }

    /// (Re)create the output duplication, detecting HDR and choosing the surface
    /// format accordingly.
    fn duplicate(device: &ID3D11Device, output_index: u32) -> Result<Duplication> {
        // DuplicateOutput1 (HDR path) requires the process to be per-monitor DPI
        // aware or it fails with DXGI_ERROR_UNSUPPORTED. Harmless if already set.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let dxgi_device: IDXGIDevice = device.cast().context("Failed to cast to IDXGIDevice")?;
        let adapter = unsafe { dxgi_device.GetAdapter().context("GetAdapter failed")? };
        let output = unsafe {
            adapter.EnumOutputs(output_index).context(format!(
                "EnumOutputs({}) failed — check monitor index",
                output_index
            ))?
        };
        let desc = unsafe { output.GetDesc().context("GetDesc failed")? };
        let rect = desc.DesktopCoordinates;
        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;

        // HDR detection via IDXGIOutput6 (Windows 10 1703+). Absence => SDR.
        let hdr = output
            .cast::<IDXGIOutput6>()
            .ok()
            .and_then(|o6| unsafe { o6.GetDesc1().ok() })
            .map(|d1| d1.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)
            .unwrap_or(false);

        let (duplication, sdr_white_scale) = if hdr {
            // Request FP16 scRGB so HDR data isn't clipped/garbled by DXGI.
            let output5: IDXGIOutput5 = output
                .cast()
                .context("IDXGIOutput5 required for HDR capture")?;
            let dup = unsafe {
                output5
                    .DuplicateOutput1(device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT])
                    .context("DuplicateOutput1 (HDR) failed")?
            };
            let scale = sdr_white_scale_for(&desc.DeviceName);
            info!("HDR SDR-white scale: {:.3} (scRGB)", scale);
            (dup, scale)
        } else {
            let output1: IDXGIOutput1 = output
                .cast()
                .context("Failed to cast to IDXGIOutput1 — DXGI 1.2+ required")?;
            let dup = unsafe {
                output1
                    .DuplicateOutput(device)
                    .context("DuplicateOutput failed — is another app using Desktop Duplication?")?
            };
            (dup, 1.0)
        };

        Ok(Duplication {
            duplication,
            width,
            height,
            hdr,
            sdr_white_scale,
        })
    }

    /// The shared D3D11 device, wrapped by FFmpeg's hw device and reused by the converter.
    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// The immediate context, used for the GPU->GPU copies into the frame pool.
    pub fn context(&self) -> &ID3D11DeviceContext {
        &self.context
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// True when the captured surface is FP16 scRGB and needs HDR->SDR tone-mapping.
    pub fn is_hdr(&self) -> bool {
        self.hdr
    }

    /// scRGB value of SDR reference white for HDR tone-mapping.
    pub fn sdr_white_scale(&self) -> f32 {
        self.sdr_white_scale
    }

    /// Acquire the latest desktop frame (0ms = non-blocking poll).
    ///
    /// Returns `Ok(Some(frame))` with a guard holding the live texture — copy out
    /// of it, then drop the guard to release it. `Ok(None)` on timeout (desktop
    /// unchanged), `Err` on access lost.
    pub fn acquire(&self) -> Result<Option<AcquiredFrame>> {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;

        let hr = unsafe {
            self.duplication
                .AcquireNextFrame(0, &mut frame_info, &mut resource)
        };
        match hr {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                anyhow::bail!("DXGI access lost — desktop duplication needs reconnect");
            }
            Err(e) => return Err(e.into()),
        }

        let resource = resource.context("AcquireNextFrame returned null resource")?;
        let texture: ID3D11Texture2D = resource.cast().context("Failed to cast to Texture2D")?;
        Ok(Some(AcquiredFrame {
            duplication: self.duplication.clone(),
            texture,
        }))
    }

    /// Reconnect after access lost (e.g. resolution change, lock screen).
    ///
    /// ponytail: if the display's HDR mode or format changes here, the caller's
    /// converter (built once at start) won't match and copies will error into
    /// another reconnect. Rebuild the converter on HDR toggle if that matters.
    pub fn reconnect(&mut self, output_index: u32) -> Result<()> {
        let dup = Self::duplicate(&self.device, output_index)?;
        self.duplication = dup.duplication;
        if dup.width != self.width || dup.height != self.height {
            warn!(
                "Resolution changed: {}x{} -> {}x{}",
                self.width, self.height, dup.width, dup.height
            );
            self.width = dup.width;
            self.height = dup.height;
        }
        self.hdr = dup.hdr;
        self.sdr_white_scale = dup.sdr_white_scale;
        debug!(
            "DXGI reconnected: {}x{} hdr={}",
            self.width, self.height, self.hdr
        );
        Ok(())
    }
}

struct Duplication {
    duplication: IDXGIOutputDuplication,
    width: u32,
    height: u32,
    hdr: bool,
    sdr_white_scale: f32,
}

/// A live duplicated desktop frame. `ReleaseFrame` is called on drop, so a new
/// frame can be acquired. The caller must copy out of `texture` before dropping.
/// Holds its own ref to the duplication so it doesn't borrow the capture (which
/// must stay mutably reconnectable).
pub struct AcquiredFrame {
    duplication: IDXGIOutputDuplication,
    pub texture: ID3D11Texture2D,
}

impl Drop for AcquiredFrame {
    fn drop(&mut self) {
        unsafe {
            self.duplication.ReleaseFrame().ok();
        }
    }
}

/// Query the SDR reference-white level for the monitor with the given GDI device
/// name, returned as an scRGB scale (nits / 80). Defaults to 1.0 (80 nits) on any
/// failure — the Windows "SDR content brightness" slider moves this, and getting
/// it right is what keeps tone-mapped output at the brightness the user sees.
fn sdr_white_scale_for(gdi_device_name: &[u16; 32]) -> f32 {
    unsafe {
        let mut num_paths = 0u32;
        let mut num_modes = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut num_paths, &mut num_modes)
            .is_err()
        {
            return 1.0;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); num_paths as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); num_modes as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut num_paths,
            paths.as_mut_ptr(),
            &mut num_modes,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return 1.0;
        }

        for path in paths.iter().take(num_paths as usize) {
            // Resolve this path's source GDI device name (\\.\DISPLAYn).
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: path.sourceInfo.adapterId,
                    id: path.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            if src.viewGdiDeviceName != *gdi_device_name {
                continue;
            }

            // Matched our monitor — read its SDR white level off the target.
            let mut wl = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: path.targetInfo.adapterId,
                    id: path.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut wl.header) != 0 {
                return 1.0;
            }
            // SDRWhiteLevel is scaled so nits = level / 1000 * 80; the scRGB value
            // of SDR white is therefore level / 1000.
            let scale = wl.SDRWhiteLevel as f32 / 1000.0;
            return if scale > 0.0 { scale } else { 1.0 };
        }
    }
    1.0
}

/// The DXGI format of the captured surface (for the converter's source texture).
pub fn capture_format(hdr: bool) -> DXGI_FORMAT {
    if hdr {
        DXGI_FORMAT_R16G16B16A16_FLOAT
    } else {
        DXGI_FORMAT_B8G8R8A8_UNORM
    }
}
