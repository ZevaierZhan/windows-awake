//! Hardware D3D11 rendering with premultiplied alpha; no GDI animation or audio stream.
use crate::ink_asset::{Asset, frame_pair, smooth};
use std::{ffi::c_void, mem::size_of};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::Fxc::*, Direct3D::*, Direct3D11::*, DirectComposition::*, Dxgi::Common::*,
            Dxgi::*,
        },
    },
    core::{Interface, PCSTR, s},
};

fn bytes<T>(value: &T) -> &[u8] {
    // Only called for initialized POD shader constants, never COM objects.
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) }
}
fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<Vec<u8>> {
    let source = include_bytes!("ink.hlsl");
    let mut blob = None;
    unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            s!("ink.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut blob,
            None,
        )?;
        let blob = blob.unwrap();
        Ok(
            std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize())
                .to_vec(),
        )
    }
}
pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    asset: Asset,
    frames: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    constants: ID3D11Buffer,
    vertex: ID3D11VertexShader,
    pixel: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    loaded: [u32; 8],
    fraction: [f32; 4],
}
impl Gpu {
    pub fn new() -> Result<Self, String> {
        Self::create().map_err(|e| format!("GPU 水墨不可用，跳过动画与熄屏：{e}"))
    }
    fn create() -> windows::core::Result<Self> {
        unsafe {
            let mut device = None;
            let mut context = None;
            // Fail closed on unavailable hardware instead of running a costly software renderer.
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
            let device = device.unwrap();
            let context = context.unwrap();
            let asset = Asset::new()
                .map_err(|e| windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e))?;
            let desc = D3D11_TEXTURE2D_DESC {
                Width: asset.width,
                Height: asset.height,
                MipLevels: 1,
                ArraySize: 8,
                Format: DXGI_FORMAT_R8G8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            };
            let mut frames = None;
            device.CreateTexture2D(&desc, None, Some(&mut frames))?;
            let frames = frames.unwrap();
            let mut view = None;
            device.CreateShaderResourceView(&frames, None, Some(&mut view))?;
            let buffer = D3D11_BUFFER_DESC {
                ByteWidth: 32,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let mut constants = None;
            device.CreateBuffer(&buffer, None, Some(&mut constants))?;
            let mut vertex = None;
            device.CreateVertexShader(
                &compile(s!("vs"), s!("vs_5_0"))?,
                None,
                Some(&mut vertex),
            )?;
            let mut pixel = None;
            device.CreatePixelShader(&compile(s!("ps"), s!("ps_5_0"))?, None, Some(&mut pixel))?;
            let desc = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_BORDER,
                AddressV: D3D11_TEXTURE_ADDRESS_BORDER,
                AddressW: D3D11_TEXTURE_ADDRESS_BORDER,
                ComparisonFunc: D3D11_COMPARISON_NEVER,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut sampler = None;
            device.CreateSamplerState(&desc, Some(&mut sampler))?;
            Ok(Self {
                device,
                context,
                asset,
                frames,
                view: view.unwrap(),
                constants: constants.unwrap(),
                vertex: vertex.unwrap(),
                pixel: pixel.unwrap(),
                sampler: sampler.unwrap(),
                loaded: [u32::MAX; 8],
                fraction: [0.; 4],
            })
        }
    }
    pub fn prepare(&mut self, p: f32) -> Result<(), String> {
        for corner in 0..4 {
            let (a, b, t) = frame_pair(p, corner, self.asset.count);
            self.fraction[corner] = t;
            for (offset, index) in [a, b].into_iter().enumerate() {
                let slot = corner * 2 + offset;
                if self.loaded[slot] != index {
                    let pixels = self.asset.frame(index)?;
                    unsafe {
                        self.context.UpdateSubresource(
                            &self.frames,
                            slot as u32,
                            None,
                            pixels.as_ptr().cast(),
                            self.asset.width * 2,
                            0,
                        );
                    }
                    self.loaded[slot] = index;
                }
            }
        }
        Ok(())
    }
    pub fn draw(&self, target: &ID3D11RenderTargetView, w: u32, h: u32, p: f32) {
        let state = [
            p,
            smooth(p),
            w as f32 / h as f32,
            0.,
            self.fraction[0],
            self.fraction[1],
            self.fraction[2],
            self.fraction[3],
        ];
        unsafe {
            self.context.UpdateSubresource(
                &self.constants,
                0,
                None,
                bytes(&state).as_ptr().cast(),
                0,
                0,
            );
            self.context
                .OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            self.context.RSSetViewports(Some(&[D3D11_VIEWPORT {
                Width: w as f32,
                Height: h as f32,
                MaxDepth: 1.,
                ..Default::default()
            }]));
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.VSSetShader(&self.vertex, None);
            self.context.PSSetShader(&self.pixel, None);
            self.context
                .PSSetConstantBuffers(0, Some(&[Some(self.constants.clone())]));
            self.context
                .PSSetShaderResources(0, Some(&[Some(self.view.clone())]));
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context.Draw(3, 0);
        }
    }
    pub fn surface(&self, w: u32, h: u32) -> windows::core::Result<Surface> {
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
            let mut texture = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?;
            let texture = texture.unwrap();
            let mut target = None;
            self.device
                .CreateRenderTargetView(&texture, None, Some(&mut target))?;
            let desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..desc
            };
            let mut readback = None;
            self.device
                .CreateTexture2D(&desc, None, Some(&mut readback))?;
            Ok(Surface {
                texture,
                target: target.unwrap(),
                readback: readback.unwrap(),
                width: w,
                height: h,
            })
        }
    }
    pub fn read(&self, surface: &Surface) -> windows::core::Result<Vec<u8>> {
        unsafe {
            self.context
                .CopyResource(&surface.readback, &surface.texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&surface.readback, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut pixels = vec![0; surface.width as usize * surface.height as usize * 4];
            for y in 0..surface.height as usize {
                let row = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    surface.width as usize * 4,
                );
                pixels[y * surface.width as usize * 4..(y + 1) * surface.width as usize * 4]
                    .copy_from_slice(row);
            }
            self.context.Unmap(&surface.readback, 0);
            Ok(pixels)
        }
    }
}
pub struct Surface {
    texture: ID3D11Texture2D,
    pub target: ID3D11RenderTargetView,
    readback: ID3D11Texture2D,
    width: u32,
    height: u32,
}
pub struct Composition {
    pub swap: IDXGISwapChain1,
    target: ID3D11RenderTargetView,
    // Retain the visual tree and target for the lifetime of its HWND.
    _device: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    width: u32,
    height: u32,
}
impl Composition {
    pub fn new(gpu: &Gpu, hwnd: *mut c_void, w: u32, h: u32) -> windows::core::Result<Self> {
        unsafe {
            let dxgi: IDXGIDevice = gpu.device.cast()?;
            let adapter = dxgi.GetAdapter()?;
            let factory: IDXGIFactory2 = adapter.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w,
                Height: h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&gpu.device, &desc, None)?;
            let texture: ID3D11Texture2D = swap.GetBuffer(0)?;
            let mut target = None;
            gpu.device
                .CreateRenderTargetView(&texture, None, Some(&mut target))?;
            let device: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let composition_target = device.CreateTargetForHwnd(HWND(hwnd), true)?;
            let visual = device.CreateVisual()?;
            visual.SetContent(&swap)?;
            composition_target.SetRoot(&visual)?;
            device.Commit()?;
            Ok(Self {
                swap,
                target: target.unwrap(),
                _device: device,
                _target: composition_target,
                _visual: visual,
                width: w,
                height: h,
            })
        }
    }
    pub fn present(&self, gpu: &Gpu, p: f32) -> windows::core::Result<()> {
        gpu.draw(&self.target, self.width, self.height, p);
        // The desktop compositor may still own both buffers. Skip this frame rather
        // than block input cancellation or interpret a temporary busy buffer as failure.
        let result = unsafe { self.swap.Present(0, DXGI_PRESENT_DO_NOT_WAIT) };
        if result == DXGI_ERROR_WAS_STILL_DRAWING {
            Ok(())
        } else {
            result.ok()
        }
    }
}
