// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! FFmpeg D3D11 hardware device + frame pool for zero-copy GPU encoding.
//!
//! Wraps the capture's `ID3D11Device` in an `AVHWDeviceContext` (D3D11VA) and an
//! `AVHWFramesContext` of BGRA GPU textures. Captured desktop textures are copied
//! GPU->GPU into pool frames and handed straight to the hardware encoder, which
//! does RGB->NV12 internally — no CPU readback, no re-upload.

use anyhow::{Result, bail};
use ffmpeg_sys_next as ff;
use std::mem::ManuallyDrop;
use std::os::raw::c_void;
use windows::Win32::Graphics::Direct3D11::{D3D11_BIND_RENDER_TARGET, ID3D11Device};
use windows::core::Interface;

/// FFmpeg's `AVD3D11VADeviceContext` (hwcontext_d3d11va.h). ffmpeg-sys-next does
/// not generate this binding, so we mirror its (stable) ABI layout. All COM
/// pointers are opaque to us.
#[repr(C)]
struct AVD3D11VADeviceContext {
    device: *mut c_void,         // ID3D11Device*
    device_context: *mut c_void, // ID3D11DeviceContext*
    video_device: *mut c_void,   // ID3D11VideoDevice*
    video_context: *mut c_void,  // ID3D11VideoContext*
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

/// FFmpeg's `AVD3D11VAFramesContext`. `BindFlags`/`MiscFlags` are D3D11 `UINT`s
/// applied to every texture the pool allocates.
#[repr(C)]
struct AVD3D11VAFramesContext {
    texture: *mut c_void, // ID3D11Texture2D*
    bind_flags: u32,
    misc_flags: u32,
}

/// Owns the FFmpeg hardware device + BGRA frame pool for one capture session.
pub struct HwFrames {
    device_ref: *mut ff::AVBufferRef,
    frames_ref: *mut ff::AVBufferRef,
}

impl HwFrames {
    /// Wrap `device` in a D3D11VA hw device and a BGRA frame pool at `enc_w`x`enc_h`.
    pub fn new(device: &ID3D11Device, enc_w: u32, enc_h: u32) -> Result<Self> {
        unsafe {
            let device_ref =
                ff::av_hwdevice_ctx_alloc(ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA);
            if device_ref.is_null() {
                bail!("av_hwdevice_ctx_alloc(D3D11VA) failed — FFmpeg lacks d3d11va support");
            }
            let mut this = Self {
                device_ref,
                frames_ref: std::ptr::null_mut(),
            };

            // Inject our device. FFmpeg fetches the immediate context + video device
            // and enables multithread protection during init. Hand it an owned
            // reference (it Releases on uninit), so AddRef via clone + leak.
            let hwdev = (*device_ref).data as *mut ff::AVHWDeviceContext;
            let d3dctx = (*hwdev).hwctx as *mut AVD3D11VADeviceContext;
            let owned = ManuallyDrop::new(device.clone());
            (*d3dctx).device = owned.as_raw();

            let ret = ff::av_hwdevice_ctx_init(device_ref);
            if ret < 0 {
                bail!("av_hwdevice_ctx_init failed ({ret})");
            }

            // BGRA frame pool, render-target bindable so the encoder can register
            // each texture (and the scaler can target it).
            let frames_ref = ff::av_hwframe_ctx_alloc(device_ref);
            if frames_ref.is_null() {
                bail!("av_hwframe_ctx_alloc failed");
            }
            this.frames_ref = frames_ref;

            let fc = (*frames_ref).data as *mut ff::AVHWFramesContext;
            (*fc).format = ff::AVPixelFormat::AV_PIX_FMT_D3D11;
            (*fc).sw_format = ff::AVPixelFormat::AV_PIX_FMT_BGRA;
            (*fc).width = enc_w as i32;
            (*fc).height = enc_h as i32;
            // Fixed pool: enough for the in-flight encoder frames + the one we hold
            // as `last` for the unchanged-desktop case.
            (*fc).initial_pool_size = 20;
            let d3dframes = (*fc).hwctx as *mut AVD3D11VAFramesContext;
            (*d3dframes).bind_flags = D3D11_BIND_RENDER_TARGET.0 as u32;

            let ret = ff::av_hwframe_ctx_init(frames_ref);
            if ret < 0 {
                bail!("av_hwframe_ctx_init failed ({ret}) — encoder may not accept BGRA D3D11");
            }

            Ok(this)
        }
    }

    /// The frames-context buffer ref, for the encoder's `hw_frames_ctx`.
    pub fn frames_ref(&self) -> *mut ff::AVBufferRef {
        self.frames_ref
    }

    /// Allocate a pooled D3D11 frame (BGRA texture). The caller copies into its
    /// texture/subresource, sets PTS, and submits it to the encoder.
    pub fn get_frame(&self) -> Result<HwFrame> {
        unsafe {
            let frame = ff::av_frame_alloc();
            if frame.is_null() {
                bail!("av_frame_alloc failed");
            }
            let ret = ff::av_hwframe_get_buffer(self.frames_ref, frame, 0);
            if ret < 0 {
                let mut f = frame;
                ff::av_frame_free(&mut f);
                bail!("av_hwframe_get_buffer failed ({ret}) — pool exhausted?");
            }
            Ok(HwFrame(frame))
        }
    }
}

impl Drop for HwFrames {
    fn drop(&mut self) {
        unsafe {
            if !self.frames_ref.is_null() {
                ff::av_buffer_unref(&mut self.frames_ref);
            }
            if !self.device_ref.is_null() {
                ff::av_buffer_unref(&mut self.device_ref);
            }
        }
    }
}

/// A pooled D3D11 hardware frame. `data[0]` is the pool's `ID3D11Texture2D`
/// (an array texture), `data[1]` the array slice index of this frame.
pub struct HwFrame(*mut ff::AVFrame);

impl HwFrame {
    /// The pool's `ID3D11Texture2D` (as a raw COM pointer) and this frame's
    /// subresource index within it.
    pub fn texture(&self) -> (*mut c_void, u32) {
        unsafe {
            let tex = (*self.0).data[0] as *mut c_void;
            let index = (*self.0).data[1] as usize as u32;
            (tex, index)
        }
    }

    pub fn set_pts(&mut self, pts: i64) {
        unsafe {
            (*self.0).pts = pts;
        }
    }

    pub fn as_ptr(&self) -> *mut ff::AVFrame {
        self.0
    }
}

impl Drop for HwFrame {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::av_frame_free(&mut self.0);
            }
        }
    }
}
