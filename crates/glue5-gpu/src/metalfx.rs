//! MetalFX spatial upscaling (Apple GPUs): the back buffer scaled to the
//! window by Apple's upscaler instead of the bicubic filter. It runs on the
//! submit thread, on the same queue, in a command buffer of its own (wgpu
//! does not let raw commands share an encoder with its own) submitted
//! between the game's and the present pass's. The back buffer and the
//! game's HUD go through it; the application's overlay is drawn after, at
//! the window's resolution.
//!
//! The spatial scaler only needs the finished frame. The temporal one
//! (sharper, and frame interpolation after it) needs depth, motion vectors
//! and a jittered camera from the engine: see docs/design.md.
//!
//! A scaler is made for one input and one output size, and making one costs
//! milliseconds: while the sizes keep changing (a window being resized, the
//! engine replacing its back buffer) frames take the bicubic filter, and a
//! scaler is made once they have held for a moment.

use std::time::{Duration, Instant};

use objc2::Message;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureUsage};
use objc2_metal_fx::{
    MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerColorProcessingMode, MTLFXSpatialScalerDescriptor,
};
use wgpu::hal::api::Metal;

use crate::submit::Present;

/// Sizes hold this long before a scaler is made for them.
const SETTLE: Duration = Duration::from_millis(250);

/// What a scaler is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    input: (u32, u32),
    output: (u32, u32),
    format: MTLPixelFormat,
}

struct Scaler {
    key: Key,
    fx: Retained<ProtocolObject<dyn MTLFXSpatialScaler>>,
    /// What it writes, for Metal and for the present pass.
    output: Retained<ProtocolObject<dyn MTLTexture>>,
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
}

pub struct MetalFx {
    /// None: this GPU has no MetalFX.
    device: Option<Retained<ProtocolObject<dyn MTLDevice>>>,
    scaler: Option<Scaler>,
    /// The sizes last asked for, and since when.
    wanted: Option<(Key, Instant)>,
    /// Sizes no scaler could be made for.
    failed: Option<Key>,
}

// Made on one thread (the application's, with the screen) and used on
// another (the submit thread), never at the same time; Metal's objects may
// be (wgpu's Metal backend relies on the same).
unsafe impl Send for MetalFx {}

impl MetalFx {
    pub fn new(device: &wgpu::Device) -> MetalFx {
        let raw = unsafe { device.as_hal::<Metal>() }.map(|d| d.raw_device().retain());
        let device = raw.filter(|d| unsafe { MTLFXSpatialScalerDescriptor::supportsDevice(d) });
        log::info!("gpu: MetalFX spatial upscaling {}", if device.is_some() { "available" } else { "not available" });
        MetalFx { device, scaler: None, wanted: None, failed: None }
    }

    pub fn supported(&self) -> bool {
        self.device.is_some()
    }

    /// The back buffer scaled to `size`, when `wanted` and possible now: the
    /// commands, then the image to show (at `size`, drawn 1:1).
    pub fn upscale(
        &mut self,
        device: &wgpu::Device,
        src: &Present,
        size: (u32, u32),
        wanted: bool,
    ) -> Option<(Vec<wgpu::CommandBuffer>, wgpu::TextureView)> {
        if !wanted || self.device.is_none() {
            // Its memory back while it is not used.
            self.scaler = None;
            self.wanted = None;
            return None;
        }
        let input = color_texture(&src.texture)?;
        let key = Key { input: (src.width, src.height), output: size, format: input.pixelFormat() };
        let mut buffers = Vec::new();
        if self.scaler.as_ref().is_none_or(|s| s.key != key) {
            match self.wanted {
                Some((k, since)) if k == key => {
                    if since.elapsed() < SETTLE || self.failed == Some(key) {
                        return None;
                    }
                }
                _ => {
                    self.wanted = Some((key, Instant::now()));
                    return None;
                }
            }
            self.scaler = None;
            let made = autoreleasepool(|_| self.make(device, key));
            let Some((scaler, clear)) = made else {
                log::warn!("gpu: no MetalFX scaler for {key:?}, bicubic instead");
                self.failed = Some(key);
                return None;
            };
            log::info!("gpu: MetalFX {}x{} -> {}x{}", key.input.0, key.input.1, key.output.0, key.output.1);
            self.scaler = Some(scaler);
            buffers.push(clear);
        }
        let s = self.scaler.as_ref()?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("metalfx") });
        let encoded = autoreleasepool(|_| unsafe {
            encoder.as_hal_mut::<Metal, _, _>(|e| {
                let Some(commands) = e.and_then(|e| e.raw_command_buffer()) else { return false };
                s.fx.setColorTexture(Some(&input));
                s.fx.setOutputTexture(Some(&s.output));
                s.fx.setInputContentWidth(key.input.0 as usize);
                s.fx.setInputContentHeight(key.input.1 as usize);
                s.fx.encodeToCommandBuffer(commands);
                true
            })
        });
        if !encoded {
            return None;
        }
        buffers.push(encoder.finish());
        Some((buffers, s.view.clone()))
    }

    /// A scaler for `key`, and the commands that clear its output once
    /// (wgpu would otherwise clear it, as never written, after MetalFX has).
    fn make(&self, device: &wgpu::Device, key: Key) -> Option<(Scaler, wgpu::CommandBuffer)> {
        let format = match key.format {
            MTLPixelFormat::RGBA8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            MTLPixelFormat::BGRA8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            MTLPixelFormat::RGB10A2Unorm => wgpu::TextureFormat::Rgb10a2Unorm,
            MTLPixelFormat::RGBA16Float => wgpu::TextureFormat::Rgba16Float,
            _ => return None,
        };
        let fx = unsafe {
            let desc = MTLFXSpatialScalerDescriptor::new();
            desc.setColorTextureFormat(key.format);
            desc.setOutputTextureFormat(key.format);
            desc.setInputWidth(key.input.0 as usize);
            desc.setInputHeight(key.input.1 as usize);
            desc.setOutputWidth(key.output.0 as usize);
            desc.setOutputHeight(key.output.1 as usize);
            // The back buffer holds display-ready (gamma-encoded) colour.
            desc.setColorProcessingMode(MTLFXSpatialScalerColorProcessingMode::Perceptual);
            desc.newSpatialScalerWithDevice(self.device.as_ref()?)?
        };
        // What MetalFX needs of its output (render target, shader writes),
        // and the present pass samples it.
        let needs = unsafe { fx.outputTextureUsage() };
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        if needs.contains(MTLTextureUsage::ShaderWrite) {
            usage |= wgpu::TextureUsages::STORAGE_BINDING;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("metalfx output"),
            size: wgpu::Extent3d { width: key.output.0, height: key.output.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::warn!("gpu: MetalFX output texture: {e}");
            return None;
        }
        let output = unsafe { texture.as_hal::<Metal>() }.map(|t| t.raw_handle().retain())?;
        let view = texture.create_view(&Default::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("metalfx clear") });
        drop(crate::present::begin_pass(&mut encoder, &view, wgpu::Color::BLACK));
        Some((Scaler { key, fx, output, view, _texture: texture }, encoder.finish()))
    }
}

/// The back buffer as MetalFX reads it: its stored values (an sRGB format
/// holds encoded colour, which is what the perceptual mode expects).
fn color_texture(texture: &wgpu::Texture) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    let raw = unsafe { texture.as_hal::<Metal>() }.map(|t| t.raw_handle().retain())?;
    let plain = match raw.pixelFormat() {
        MTLPixelFormat::RGBA8Unorm_sRGB => MTLPixelFormat::RGBA8Unorm,
        MTLPixelFormat::BGRA8Unorm_sRGB => MTLPixelFormat::BGRA8Unorm,
        _ => return Some(raw),
    };
    raw.newTextureViewWithPixelFormat(plain)
}
