// Copyright (c) 2026 Larson Software LLC.
// All rights reserved.
// This source code is proprietary and confidential.

//! GPU colour/scale conversion via a D3D11 pixel shader.
//!
//! Two modes, both rendering the captured texture through a fullscreen pass into
//! an encode-sized BGRA render target (the sampler does any downscale):
//!
//! - **passthrough** — SDR BGRA in, BGRA out. Used only when the desktop size
//!   differs from the encode size; colour conversion (RGB->NV12) is left to the
//!   hardware encoder.
//! - **hdr** — FP16 scRGB (linear, Rec.709, 1.0 == SDR white) in, SDR sRGB BGRA
//!   out. Normalises by the SDR-white level, clips HDR highlights, then applies
//!   the sRGB transfer function so the saved video matches the on-screen SDR look
//!   instead of coming out washed-out/too bright.

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::core::{PCSTR, s};

/// Build the HLSL, baking in the SDR-white scale used by the HDR tone-map.
fn hlsl(sdr_white: f32) -> String {
    format!(
        r#"
Texture2D src : register(t0);
SamplerState smp : register(s0);

struct VSOut {{ float4 pos : SV_POSITION; float2 uv : TEXCOORD0; }};

VSOut VSMain(uint id : SV_VertexID) {{
    VSOut o;
    o.uv  = float2((id << 1) & 2, id & 2);
    o.pos = float4(o.uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return o;
}}

// SDR: source is already sRGB-encoded BGRA, pass it through.
float4 PSCopy(VSOut i) : SV_TARGET {{
    return src.Sample(smp, i.uv);
}}

// HDR: source is scRGB linear (Rec.709, {sdr_white} == SDR white). Normalise,
// clip highlights, encode sRGB.
static const float SDR_WHITE = {sdr_white};
float4 PSTonemap(VSOut i) : SV_TARGET {{
    float3 c = max(src.Sample(smp, i.uv).rgb, 0.0);
    c /= SDR_WHITE;
    c = saturate(c);
    c = (c <= 0.0031308) ? c * 12.92 : 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    return float4(c, 1.0);
}}
"#
    )
}

/// Owns the conversion pipeline: an encode-sized BGRA render target and a
/// capture-sized source the captured texture is copied into.
pub struct Converter {
    device: ID3D11Device,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    raster: ID3D11RasterizerState,

    enc_w: u32,
    enc_h: u32,
    rt_tex: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,

    // Capture-resolution source. Recreated if the capture size changes.
    src_format: DXGI_FORMAT,
    src_w: u32,
    src_h: u32,
    src_tex: ID3D11Texture2D,
    srv: ID3D11ShaderResourceView,
}

impl Converter {
    /// SDR BGRA->BGRA scaler (used only when desktop size != encode size).
    pub fn passthrough(device: &ID3D11Device, enc_w: u32, enc_h: u32) -> Result<Self> {
        Self::new(device, enc_w, enc_h, DXGI_FORMAT_B8G8R8A8_UNORM, s!("PSCopy"), 1.0)
    }

    /// HDR FP16 scRGB -> SDR sRGB BGRA tone-mapper. `sdr_white` is the scRGB value
    /// of SDR reference white (nits / 80).
    pub fn hdr(
        device: &ID3D11Device,
        enc_w: u32,
        enc_h: u32,
        src_format: DXGI_FORMAT,
        sdr_white: f32,
    ) -> Result<Self> {
        Self::new(device, enc_w, enc_h, src_format, s!("PSTonemap"), sdr_white)
    }

    fn new(
        device: &ID3D11Device,
        enc_w: u32,
        enc_h: u32,
        src_format: DXGI_FORMAT,
        ps_entry: PCSTR,
        sdr_white: f32,
    ) -> Result<Self> {
        unsafe {
            let source = hlsl(sdr_white);
            let vs_code = compile(&source, s!("VSMain"), s!("vs_5_0"))?;
            let ps_code = compile(&source, ps_entry, s!("ps_5_0"))?;

            let mut vs = None;
            device.CreateVertexShader(&vs_code, None, Some(&mut vs))?;
            let mut ps = None;
            device.CreatePixelShader(&ps_code, None, Some(&mut ps))?;

            let sampler_desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device.CreateSamplerState(&sampler_desc, Some(&mut sampler))?;

            let raster_desc = D3D11_RASTERIZER_DESC {
                FillMode: D3D11_FILL_SOLID,
                CullMode: D3D11_CULL_NONE,
                ..Default::default()
            };
            let mut raster = None;
            device.CreateRasterizerState(&raster_desc, Some(&mut raster))?;

            let (rt_tex, rtv) = make_target(device, enc_w, enc_h)?;
            // Source recreated lazily on first convert to the real capture size.
            let (src_tex, srv) = make_source(device, enc_w, enc_h, src_format)?;

            Ok(Self {
                device: device.clone(),
                vs: vs.unwrap(),
                ps: ps.unwrap(),
                sampler: sampler.unwrap(),
                raster: raster.unwrap(),
                enc_w,
                enc_h,
                rt_tex,
                rtv: rtv.unwrap(),
                src_format,
                src_w: enc_w,
                src_h: enc_h,
                src_tex,
                srv: srv.unwrap(),
            })
        }
    }

    /// Copy `src` into the shader-readable source, render it (scaled / tone-mapped)
    /// into the encode-sized BGRA render target, and return that target for the
    /// caller to copy into the encoder frame pool.
    ///
    /// # Safety
    /// `ctx` must be the immediate context of the device this converter was built
    /// with, and `src` a texture of `src_format` from the same device.
    pub unsafe fn convert(
        &mut self,
        ctx: &ID3D11DeviceContext,
        src: &ID3D11Texture2D,
    ) -> Result<&ID3D11Texture2D> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            src.GetDesc(&mut desc);
            if desc.Width != self.src_w || desc.Height != self.src_h {
                let (tex, srv) = make_source(&self.device, desc.Width, desc.Height, self.src_format)?;
                self.src_tex = tex;
                self.srv = srv.unwrap();
                self.src_w = desc.Width;
                self.src_h = desc.Height;
            }

            ctx.CopyResource(&self.src_tex, src);

            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.RSSetState(&self.raster);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShaderResources(0, Some(&[Some(self.srv.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));

            let vp = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.enc_w as f32,
                Height: self.enc_h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            ctx.RSSetViewports(Some(&[vp]));
            ctx.OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            ctx.PSSetShader(&self.ps, None);
            ctx.Draw(3, 0);

            // Unbind so the render target can be used as a copy source.
            ctx.OMSetRenderTargets(None, None);
            ctx.PSSetShaderResources(0, Some(&[None]));
        }
        Ok(&self.rt_tex)
    }
}

/// Compile one entry point of `source` to bytecode.
unsafe fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Result<Vec<u8>> {
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        let res = D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            PCSTR::null(),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut blob,
            Some(&mut errors),
        );
        if res.is_err() {
            let msg = match &errors {
                Some(e) => {
                    let p = e.GetBufferPointer() as *const u8;
                    let n = e.GetBufferSize();
                    String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).into_owned()
                }
                None => "unknown error".to_string(),
            };
            anyhow::bail!("Shader compile failed: {msg}");
        }
        let blob = blob.context("D3DCompile returned no bytecode")?;
        let p = blob.GetBufferPointer() as *const u8;
        let n = blob.GetBufferSize();
        Ok(std::slice::from_raw_parts(p, n).to_vec())
    }
}

/// Create an encode-sized BGRA render-target texture and its RTV.
unsafe fn make_target(
    device: &ID3D11Device,
    w: u32,
    h: u32,
) -> Result<(ID3D11Texture2D, Option<ID3D11RenderTargetView>)> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut tex = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .context("Failed to create render-target texture")?;
        let tex = tex.context("render-target texture was null")?;

        let mut rtv = None;
        device.CreateRenderTargetView(&tex, None, Some(&mut rtv))?;
        Ok((tex, rtv))
    }
}

/// Create the shader-readable source texture (of `format`) and its SRV.
unsafe fn make_source(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
) -> Result<(ID3D11Texture2D, Option<ID3D11ShaderResourceView>)> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut tex = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .context("Failed to create source texture")?;
        let tex = tex.context("source texture was null")?;

        let mut srv = None;
        device.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        Ok((tex, srv))
    }
}
