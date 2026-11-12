//! Жидкое стекло на видеокарте (D3D11). Тот же конвейер, что в `cpu::render`.

use std::cell::RefCell;
use std::ffi::c_void;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
    ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::core::{PCSTR, s};

use crate::{Bitmap, GlassParams};

const SHADER_SRC: &str = include_str!("../shaders/glass.hlsl");
const MAX_BLUR: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuDriver {
    Hardware,
    Warp,
}

/// Совпадает по раскладке с `cbuffer Params` в `glass.hlsl` (4 регистра по 16 байт).
#[repr(C)]
#[derive(Clone, Copy)]
struct Constants {
    size: [f32; 2],
    radius: f32,
    bevel: f32,
    refraction: f32,
    saturation: f32,
    dispersion: f32,
    rim: f32,
    tint: [f32; 4],
    opacity: f32,
    blur_radius: i32,
    horizontal: i32,
    pad: f32,
}

/// Текстуры одного размера: подложка, два блюра, результат и буфер для чтения.
struct Targets {
    width: u32,
    height: u32,
    src: ID3D11Texture2D,
    src_srv: ID3D11ShaderResourceView,
    tmp_rtv: ID3D11RenderTargetView,
    tmp_srv: ID3D11ShaderResourceView,
    blur_rtv: ID3D11RenderTargetView,
    blur_srv: ID3D11ShaderResourceView,
    out: ID3D11Texture2D,
    out_rtv: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
}

pub struct GpuGlass {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vs: ID3D11VertexShader,
    ps_blur: ID3D11PixelShader,
    ps_glass: ID3D11PixelShader,
    cbuf: ID3D11Buffer,
    sampler: ID3D11SamplerState,
    targets: RefCell<Option<Targets>>,
}

fn compile(entry: PCSTR, target: PCSTR) -> Result<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let res = unsafe {
        D3DCompile(
            SHADER_SRC.as_ptr() as *const c_void,
            SHADER_SRC.len(),
            s!("glass.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = res {
        let msg = errors
            .map(|b| unsafe {
                String::from_utf8_lossy(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()))
                    .into_owned()
            })
            .unwrap_or_default();
        bail!("HLSL compile failed: {e}: {msg}");
    }
    code.context("D3DCompile returned no bytecode")
}

fn bytes(blob: &ID3DBlob) -> &[u8] {
    unsafe { std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()) }
}

impl GpuGlass {
    pub fn new(driver: GpuDriver) -> Result<GpuGlass> {
        let driver_type = match driver {
            GpuDriver::Hardware => D3D_DRIVER_TYPE_HARDWARE,
            GpuDriver::Warp => D3D_DRIVER_TYPE_WARP,
        };
        let (mut device, mut ctx) = (None, None);
        unsafe {
            D3D11CreateDevice(
                None,
                driver_type,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice")?;
        }
        let device = device.context("no D3D11 device")?;
        let ctx = ctx.context("no D3D11 context")?;

        let vs_blob = compile(s!("vs_main"), s!("vs_5_0"))?;
        let blur_blob = compile(s!("ps_blur"), s!("ps_5_0"))?;
        let glass_blob = compile(s!("ps_glass"), s!("ps_5_0"))?;
        let (mut vs, mut ps_blur, mut ps_glass) = (None, None, None);
        let mut cbuf = None;
        let mut sampler = None;
        unsafe {
            device.CreateVertexShader(bytes(&vs_blob), None, Some(&mut vs))?;
            device.CreatePixelShader(bytes(&blur_blob), None, Some(&mut ps_blur))?;
            device.CreatePixelShader(bytes(&glass_blob), None, Some(&mut ps_glass))?;
            let cb_desc = D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<Constants>() as u32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            device.CreateBuffer(&cb_desc, None, Some(&mut cbuf))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            device.CreateSamplerState(&sd, Some(&mut sampler))?;
        }
        Ok(GpuGlass {
            device,
            ctx,
            vs: vs.context("no vertex shader")?,
            ps_blur: ps_blur.context("no blur shader")?,
            ps_glass: ps_glass.context("no glass shader")?,
            cbuf: cbuf.context("no constant buffer")?,
            sampler: sampler.context("no sampler")?,
            targets: RefCell::new(None),
        })
    }

    fn make_targets(&self, width: u32, height: u32) -> Result<Targets> {
        let desc = |usage: D3D11_USAGE, bind: u32, cpu: u32| D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: usage,
            BindFlags: bind,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let srv_bind = D3D11_BIND_SHADER_RESOURCE.0 as u32;
        let rt_bind = D3D11_BIND_RENDER_TARGET.0 as u32;
        let tex = |d: D3D11_TEXTURE2D_DESC| -> Result<ID3D11Texture2D> {
            let mut t = None;
            unsafe { self.device.CreateTexture2D(&d, None, Some(&mut t))? };
            t.context("no texture")
        };
        let srv = |t: &ID3D11Texture2D| -> Result<ID3D11ShaderResourceView> {
            let mut v = None;
            unsafe { self.device.CreateShaderResourceView(t, None, Some(&mut v))? };
            v.context("no srv")
        };
        let rtv = |t: &ID3D11Texture2D| -> Result<ID3D11RenderTargetView> {
            let mut v = None;
            unsafe { self.device.CreateRenderTargetView(t, None, Some(&mut v))? };
            v.context("no rtv")
        };
        let src = tex(desc(D3D11_USAGE_DEFAULT, srv_bind, 0))?;
        let tmp = tex(desc(D3D11_USAGE_DEFAULT, srv_bind | rt_bind, 0))?;
        let blur = tex(desc(D3D11_USAGE_DEFAULT, srv_bind | rt_bind, 0))?;
        let out = tex(desc(D3D11_USAGE_DEFAULT, rt_bind, 0))?;
        let staging = tex(desc(D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32))?;
        Ok(Targets {
            width,
            height,
            src_srv: srv(&src)?,
            tmp_rtv: rtv(&tmp)?,
            tmp_srv: srv(&tmp)?,
            blur_rtv: rtv(&blur)?,
            blur_srv: srv(&blur)?,
            out_rtv: rtv(&out)?,
            src,
            out,
            staging,
        })
    }

    fn pass(
        &self,
        ps: &ID3D11PixelShader,
        input: &ID3D11ShaderResourceView,
        target: &ID3D11RenderTargetView,
        cb: &Constants,
        w: u32,
        h: u32,
    ) {
        unsafe {
            self.ctx.UpdateSubresource(&self.cbuf, 0, None, cb as *const Constants as *const c_void, 0, 0);
            self.ctx.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            self.ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: w as f32,
                Height: h as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]));
            self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.ctx.VSSetShader(&self.vs, None);
            self.ctx.PSSetShader(ps, None);
            self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.cbuf.clone())]));
            self.ctx.PSSetShaderResources(0, Some(&[Some(input.clone())]));
            self.ctx.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.ctx.Draw(3, 0);
            // Снять привязки, чтобы текстура не оказалась входом и выходом одновременно.
            self.ctx.PSSetShaderResources(0, Some(&[None]));
            self.ctx.OMSetRenderTargets(Some(&[None]), None);
        }
    }

    /// Стекло поверх подложки; результат того же размера, предумноженный BGRA.
    pub fn render(&self, backdrop: &Bitmap, p: &GlassParams) -> Result<Bitmap> {
        let (w, h) = (backdrop.width, backdrop.height);
        if w == 0 || h == 0 || backdrop.pixels.len() != (w as usize) * (h as usize) {
            bail!("invalid backdrop {}x{} with {} pixels", w, h, backdrop.pixels.len());
        }
        let mut slot = self.targets.borrow_mut();
        if slot.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            *slot = Some(self.make_targets(w, h)?);
        }
        let t = slot.as_ref().unwrap();

        let base = Constants {
            size: [w as f32, h as f32],
            radius: p.radius,
            bevel: p.bevel,
            refraction: p.refraction,
            saturation: p.saturation,
            dispersion: p.dispersion,
            rim: p.rim,
            tint: p.tint,
            opacity: p.opacity,
            blur_radius: p.blur_radius.min(MAX_BLUR) as i32,
            horizontal: 1,
            pad: 0.0,
        };
        unsafe {
            self.ctx.UpdateSubresource(&t.src, 0, None, backdrop.pixels.as_ptr() as *const c_void, w * 4, 0);
        }
        self.pass(&self.ps_blur, &t.src_srv, &t.tmp_rtv, &base, w, h);
        self.pass(&self.ps_blur, &t.tmp_srv, &t.blur_rtv, &Constants { horizontal: 0, ..base }, w, h);
        self.pass(&self.ps_glass, &t.blur_srv, &t.out_rtv, &base, w, h);

        let mut pixels = vec![0u32; (w as usize) * (h as usize)];
        unsafe {
            self.ctx.CopyResource(&t.staging, &t.out);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&t.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            for y in 0..h as usize {
                let row = (mapped.pData as *const u8).add(y * mapped.RowPitch as usize) as *const u32;
                std::ptr::copy_nonoverlapping(row, pixels.as_mut_ptr().add(y * w as usize), w as usize);
            }
            self.ctx.Unmap(&t.staging, 0);
        }
        Ok(Bitmap { width: w, height: h, pixels })
    }
}

