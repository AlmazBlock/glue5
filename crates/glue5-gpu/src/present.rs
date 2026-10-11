//! Presenting: the submit thread scales the engine's back buffer into the
//! window's surface (aspect preserved; MetalFX where the application asks
//! for it and the GPU has it, a bicubic filter otherwise), draws the
//! overlay and presents, and keeps frames at most two ahead of the GPU,
//! window or not. Before the game runs, the application shows its UI
//! through the same [`Output`] (see [`crate::Screen`]).
//!
//! Frames are shown at an even pace: with a frame rate set, each stays on
//! screen for at least its share of a second, measured from when the one
//! before actually appeared. A display faster than the game (120 Hz
//! ProMotion, a 144 Hz monitor) would otherwise show each frame at the first
//! refresh after it is ready, for 1, 2 or 3 refreshes as it comes: judder,
//! though the frames themselves are on time. On Apple platforms the window's
//! images are taken from its Metal layer and shown by hand, as wgpu does but
//! with `presentDrawable:afterMinimumDuration:`, and the times they reached
//! the screen go into the statistics.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
#[cfg(target_vendor = "apple")]
use objc2::rc::{Retained, autoreleasepool};
#[cfg(target_vendor = "apple")]
use objc2::runtime::ProtocolObject;
#[cfg(target_vendor = "apple")]
use objc2_quartz_core::CAMetalDrawable;
use wgpu::TextureFormat;
#[cfg(target_vendor = "apple")]
use wgpu::hal::api::Metal;

use crate::done::Work;
use crate::exec::Gpu;
use crate::overlay::OverlayRenderer;
use crate::submit::Present;

/// Scales the back buffer into the window.
const PRESENT_WGSL: &str = "@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct V { @builtin(position) p: vec4f, @location(0) uv: vec2f }
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
  var p = array<vec2f, 3>(vec2f(-1, -1), vec2f(3, -1), vec2f(-1, 3));
  return V(vec4f(p[i], 0, 1), p[i] * vec2f(0.5, -0.5) + 0.5);
}
// Catmull-Rom bicubic in five bilinear taps (the corners dropped): sharp
// where bilinear blurs, e.g. 720p scaled to a phone's or a Retina display.
fn bicubic(uv: vec2f) -> vec3f {
  let size = vec2f(textureDimensions(src, 0));
  let pos = uv * size;
  let center = floor(pos - 0.5) + 0.5;
  let f = pos - center;
  let f2 = f * f;
  let f3 = f2 * f;
  let w0 = -0.5 * f3 + f2 - 0.5 * f;
  let w1 = 1.5 * f3 - 2.5 * f2 + 1.0;
  let w2 = -1.5 * f3 + 2.0 * f2 + 0.5 * f;
  let w3 = 0.5 * f3 - 0.5 * f2;
  let w12 = w1 + w2;
  let t12 = (center + w2 / w12) / size;
  let t0 = (center - 1.0) / size;
  let t3 = (center + 2.0) / size;
  let c = textureSampleLevel(src, smp, vec2f(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y)
    + textureSampleLevel(src, smp, vec2f(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y)
    + textureSampleLevel(src, smp, t12, 0.0).rgb * (w12.x * w12.y)
    + textureSampleLevel(src, smp, vec2f(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y)
    + textureSampleLevel(src, smp, vec2f(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
  let total = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
  return max(c / total, vec3f(0.0));
}
@fragment fn fs(v: V) -> @location(0) vec4f { return vec4f(bicubic(v.uv), 1.0); }";

/// Behind the application's UI while no game frame is shown.
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.05, g: 0.055, b: 0.07, a: 1.0 };

/// How the back buffer is scaled up to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upscaler {
    /// Catmull-Rom bicubic filtering.
    Bicubic = 0,
    /// MetalFX's spatial upscaler, on Apple GPUs that have it (bicubic
    /// elsewhere).
    MetalFx = 1,
}

/// How frames are shown: set by the application whenever it likes, read by
/// the renderer at each frame.
#[derive(Clone)]
pub struct Presentation(Arc<Shared>);

struct Shared {
    /// The window's size in pixels, width << 32 | height.
    size: AtomicU64,
    vsync: AtomicBool,
    metal_hud: AtomicBool,
    upscaler: AtomicU8,
    /// Frames per second at most, shown evenly (0: as they come).
    frame_rate: AtomicU32,
}

impl Presentation {
    pub fn new((width, height): (u32, u32), vsync: bool, upscaler: Upscaler) -> Self {
        let p = Presentation(Arc::new(Shared {
            size: AtomicU64::new(0),
            vsync: AtomicBool::new(vsync),
            metal_hud: AtomicBool::new(false),
            upscaler: AtomicU8::new(upscaler as u8),
            frame_rate: AtomicU32::new(0),
        }));
        p.set_size(width, height);
        p
    }

    /// The window's size in pixels.
    pub fn set_size(&self, width: u32, height: u32) {
        self.0.size.store((width as u64) << 32 | height as u64, Ordering::Relaxed);
    }

    pub fn size(&self) -> (u32, u32) {
        let v = self.0.size.load(Ordering::Relaxed);
        ((v >> 32) as u32, v as u32)
    }

    pub fn set_vsync(&self, on: bool) {
        self.0.vsync.store(on, Ordering::Relaxed);
    }

    /// Shows or hides Apple's Metal performance HUD from the next frame, once
    /// [`crate::enable_metal_hud`] has shown it.
    pub fn set_metal_hud(&self, on: bool) {
        self.0.metal_hud.store(on, Ordering::Relaxed);
    }

    fn vsync(&self) -> bool {
        self.0.vsync.load(Ordering::Relaxed)
    }

    pub fn set_upscaler(&self, upscaler: Upscaler) {
        self.0.upscaler.store(upscaler as u8, Ordering::Relaxed);
    }

    fn upscaler(&self) -> Upscaler {
        match self.0.upscaler.load(Ordering::Relaxed) {
            1 => Upscaler::MetalFx,
            _ => Upscaler::Bicubic,
        }
    }

    /// Frames per second at most, each shown for the same time (0: each
    /// as soon as it is ready). Where the platform cannot pace frames
    /// (wgpu's surface), the engine's own limiter does it alone.
    pub fn set_frame_rate(&self, fps: u32) {
        self.0.frame_rate.store(fps, Ordering::Relaxed);
    }

    /// How long each frame stays on screen at least.
    #[cfg(target_vendor = "apple")]
    fn frame_time(&self) -> Option<Duration> {
        let fps = self.0.frame_rate.load(Ordering::Relaxed);
        (fps > 0).then(|| Duration::from_secs_f64(1.0 / fps as f64))
    }
}

/// When the window's frames reached the screen (host seconds), since the
/// statistics last took them.
static ON_SCREEN: Mutex<Vec<f64>> = Mutex::new(Vec::new());

/// The times frames stayed on screen, in milliseconds, since the last call.
pub(crate) fn take_on_screen() -> Vec<f32> {
    let mut times = ON_SCREEN.lock().unwrap();
    let ms = times.windows(2).map(|w| ((w[1] - w[0]) * 1000.0) as f32).collect();
    // The last one starts the next interval.
    let last = times.last().copied();
    times.clear();
    times.extend(last);
    ms
}

/// Whether the window showing `layer` is at least partly visible: the
/// drawable of a hidden window waits up to a second for the screen (wgpu's
/// own acquisition makes the same check, gfx-rs/wgpu#8309).
#[cfg(target_os = "macos")]
fn window_visible(layer: &objc2_quartz_core::CAMetalLayer) -> bool {
    use objc2::Message;
    use objc2::runtime::NSObject;
    const VISIBLE: usize = 1 << 1; // NSWindowOcclusionStateVisible
    // The first layer with a delegate is the window's view.
    let mut current = Some(layer.retain().into_super());
    while let Some(l) = current {
        if let Some(view) = l.delegate() {
            let window: Option<Retained<NSObject>> = unsafe { objc2::msg_send![&*view, window] };
            let state: usize = window.map_or(VISIBLE, |w| unsafe { objc2::msg_send![&*w, occlusionState] });
            return state & VISIBLE != 0;
        }
        current = l.superlayer();
    }
    true
}

/// The window's next image, to draw into and then show ([`Output::show`]).
pub struct Frame {
    pub texture: wgpu::Texture,
    /// The format to view it with (the surface's, without sRGB).
    pub format: TextureFormat,
    #[cfg(not(target_vendor = "apple"))]
    surface: wgpu::SurfaceTexture,
    #[cfg(target_vendor = "apple")]
    drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
}

/// A window to present into.
pub struct Window {
    pub instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub presentation: Presentation,
    /// Drawn over the frames (the application's UI).
    pub overlay: crate::Overlay,
}

/// Where a `src`-sized image goes in a `dst`-sized frame (x, y, width,
/// height in pixels): as large as fits, aspect preserved, centred. Aspects
/// within 1% fill the frame: a band one pixel wide would look like a fault,
/// a 1% stretch does not show.
fn fit((sw, sh): (u32, u32), (dw, dh): (u32, u32)) -> (u32, u32, u32, u32) {
    let (sw, sh) = (sw.max(1) as f64, sh.max(1) as f64);
    if ((sw / sh) / (dw as f64 / dh as f64) - 1.0).abs() < 0.01 {
        return (0, 0, dw, dh);
    }
    let scale = (dw as f64 / sw).min(dh as f64 / sh);
    let w = ((sw * scale).round() as u32).clamp(1, dw);
    let h = ((sh * scale).round() as u32).clamp(1, dh);
    ((dw - w) / 2, (dh - h) / 2, w, h)
}

/// The window's surface.
struct Display {
    surface: wgpu::Surface<'static>,
    /// Size and vsync it is configured with.
    configured: Option<(u32, u32, bool)>,
    /// The surface's format, and the one its images are viewed with (no sRGB).
    surface_format: Option<TextureFormat>,
    format: Option<TextureFormat>,
    blit: Option<(wgpu::RenderPipeline, wgpu::Sampler)>,
    /// Whether the layer last asked for the HUD shown.
    #[cfg(target_vendor = "apple")]
    metal_hud: Option<bool>,
}

impl Display {
    fn configure(&mut self, caps: &wgpu::SurfaceCapabilities, device: &wgpu::Device, (w, h, vsync): (u32, u32, bool)) {
        // The back buffer holds display-ready (gamma-encoded) values: a non-sRGB surface.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm))
            .unwrap_or(caps.formats[0]);
        let linear = format.remove_srgb_suffix();
        let present_mode = if vsync {
            wgpu::PresentMode::Fifo
        } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        // (Waits for the GPU to finish what it has: a window being resized
        // pays it once per frame.)
        self.surface.configure(
            device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: Default::default(),
                width: w,
                height: h,
                present_mode,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: if linear != format { vec![linear] } else { vec![] },
            },
        );
        if self.format != Some(linear) {
            self.blit = None;
        }
        self.surface_format = Some(format);
        self.format = Some(linear);
        self.configured = Some((w, h, vsync));
        log::debug!("gpu: surface {w}x{h} {format:?} {present_mode:?}");
    }

    fn acquire(
        &mut self,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        wanted: (u32, u32, bool),
        metal_hud: bool,
    ) -> Option<Frame> {
        if wanted.0 == 0 || wanted.1 == 0 {
            return None;
        }
        if self.configured != Some(wanted) {
            let caps = self.surface.get_capabilities(adapter);
            if caps.formats.is_empty() {
                return None;
            }
            self.configure(&caps, device, wanted);
        }
        self.next_image(device, metal_hud)
    }

    #[cfg(not(target_vendor = "apple"))]
    fn next_image(&mut self, _: &wgpu::Device, _: bool) -> Option<Frame> {
        let format = self.format?;
        let surface = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                self.configured = None;
                t
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.configured = None;
                return None;
            }
            _ => return None,
        };
        Some(Frame { texture: surface.texture.clone(), format, surface })
    }

    /// The layer's next drawable (waiting for one, as wgpu does), as a
    /// texture to draw into.
    #[cfg(target_vendor = "apple")]
    fn next_image(&mut self, device: &wgpu::Device, metal_hud: bool) -> Option<Frame> {
        use objc2_metal::{MTLTexture, MTLTextureType};
        let (surface_format, format) = (self.surface_format?, self.format?);
        let layer = unsafe { self.surface.as_hal::<Metal>() }.map(|s| s.render_layer().lock().clone())?;
        // Followed once the HUD `crate::enable_metal_hud` showed is attached.
        if self.metal_hud != Some(metal_hud) {
            use objc2_foundation::{NSDictionary, ns_string};
            let mode = if metal_hud { ns_string!("default") } else { ns_string!("disabled") };
            let properties = NSDictionary::from_slices(&[ns_string!("mode")], &[mode]);
            // SAFETY: HUD properties are string keys and string values; the cast
            // only erases NSDictionary's generic types for the Objective-C API.
            let properties = unsafe { Retained::cast_unchecked(properties) };
            autoreleasepool(|_| unsafe { layer.setDeveloperHUDProperties(Some(&properties)) });
            self.metal_hud = Some(metal_hud);
        }
        #[cfg(target_os = "macos")]
        if !window_visible(&layer) {
            return None;
        }
        let drawable = autoreleasepool(|_| layer.nextDrawable())?;
        let raw = drawable.texture();
        let size = wgpu::Extent3d { width: raw.width() as u32, height: raw.height() as u32, depth_or_array_layers: 1 };
        let extent = wgpu::hal::CopyExtent { width: size.width, height: size.height, depth: 1 };
        let hal = unsafe {
            wgpu::hal::metal::Device::texture_from_raw(raw, surface_format, MTLTextureType::Type2D, 1, 1, extent, None)
        };
        let view_formats = if format != surface_format { vec![format] } else { vec![] };
        let texture = unsafe {
            device.create_texture_from_hal::<Metal>(
                hal,
                &wgpu::TextureDescriptor {
                    label: Some("window"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: surface_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &view_formats,
                },
                wgpu::wgt::TextureUses::UNINITIALIZED,
            )
        };
        Some(Frame { texture, format, drawable })
    }

    /// `src` scaled into `rect` (x, y, width, height) of `target`, black around it.
    fn blit(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        target: &wgpu::TextureView,
        format: TextureFormat,
        (x, y, w, h): (u32, u32, u32, u32),
    ) {
        if self.blit.is_none() {
            let module = crate::shader::create_module(device, "present", PRESENT_WGSL).ok();
            let Some(module) = module else { return };
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("present"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            self.blit = Some((pipeline, sampler));
        }
        let (pipeline, sampler) = self.blit.clone().unwrap();
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let mut pass = begin_pass(encoder, target, wgpu::Color::BLACK);
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

pub(crate) fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    clear: wgpu::Color,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    })
}

/// The window's side of presenting: its surface, the overlay drawn over the
/// frames, the upscaler. The application has it before the game runs
/// ([`crate::Screen`]), the submit thread after.
pub struct Output {
    adapter: wgpu::Adapter,
    presentation: Presentation,
    display: Display,
    overlay: OverlayRenderer,
    #[cfg(target_vendor = "apple")]
    metalfx: crate::metalfx::MetalFx,
}

impl Output {
    pub fn new(adapter: wgpu::Adapter, device: &wgpu::Device, window: Window) -> Output {
        #[cfg(not(target_vendor = "apple"))]
        let _ = device;
        Output {
            adapter,
            presentation: window.presentation,
            display: Display {
                surface: window.surface,
                configured: None,
                surface_format: None,
                format: None,
                blit: None,
                #[cfg(target_vendor = "apple")]
                metal_hud: None,
            },
            overlay: OverlayRenderer::new(window.overlay),
            #[cfg(target_vendor = "apple")]
            metalfx: crate::metalfx::MetalFx::new(device),
        }
    }

    /// Whether this GPU has MetalFX upscaling.
    pub fn has_metalfx(&self) -> bool {
        #[cfg(target_vendor = "apple")]
        return self.metalfx.supported();
        #[cfg(not(target_vendor = "apple"))]
        false
    }

    /// The next frame to draw into (none while the window has no area or
    /// its surface is being replaced).
    pub fn acquire(&mut self, device: &wgpu::Device) -> Option<Frame> {
        let (w, h) = self.presentation.size();
        self.display.acquire(
            &self.adapter,
            device,
            (w, h, self.presentation.vsync()),
            self.presentation.0.metal_hud.load(Ordering::Relaxed),
        )
    }

    /// Shows `frame`, drawn by the commands submitted before.
    #[cfg(not(target_vendor = "apple"))]
    pub fn show(&mut self, queue: &wgpu::Queue, frame: Frame) {
        drop(frame.texture);
        queue.present(frame.surface);
    }

    /// Shows `frame`, drawn by the commands submitted before: what wgpu's
    /// present does (a command buffer of its own that presents the
    /// drawable, after those on the same queue), paced by the frame rate.
    #[cfg(target_vendor = "apple")]
    pub fn show(&mut self, queue: &wgpu::Queue, frame: Frame) {
        use block2::RcBlock;
        use objc2_metal::{MTLCommandBuffer, MTLCommandQueue, MTLDrawable};
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*frame.drawable);
        let on_screen = RcBlock::new(|d: std::ptr::NonNull<ProtocolObject<dyn MTLDrawable>>| {
            // 0: skipped (a newer frame replaced it).
            let at = unsafe { d.as_ref() }.presentedTime();
            if at > 0.0 {
                ON_SCREEN.lock().unwrap().push(at);
            }
        });
        unsafe { drawable.addPresentedHandler(RcBlock::as_ptr(&on_screen)) };
        let Some(queue) = (unsafe { queue.as_hal::<Metal>() }) else { return };
        let frame_time = self.presentation.frame_time();
        autoreleasepool(|_| {
            let Some(commands) = queue.as_raw().commandBuffer() else { return };
            match frame_time {
                Some(t) => commands.presentDrawable_afterMinimumDuration(drawable, t.as_secs_f64()),
                None => commands.presentDrawable(drawable),
            }
            commands.commit();
        });
    }

    /// The commands that draw `frame`: the back buffer `src` scaled into it
    /// (without one, the background colour), then the overlay.
    pub fn compose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
        src: Option<&Present>,
    ) -> Vec<wgpu::CommandBuffer> {
        let format = frame.format;
        let target =
            frame.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(format), ..Default::default() });
        let size = (frame.texture.width(), frame.texture.height());
        let mut buffers = Vec::new();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("present") });
        match src {
            Some(p) => {
                let rect = fit((p.width, p.height), size);
                let view = match self.upscale(device, p, (rect.2, rect.3)) {
                    Some((commands, view)) => {
                        buffers.extend(commands);
                        view
                    }
                    None => p.view.clone(),
                };
                self.display.blit(device, &mut encoder, &view, &target, format, rect);
            }
            None => drop(begin_pass(&mut encoder, &target, BACKGROUND)),
        }
        // The application's UI over it (its queue writes go before the submit).
        self.overlay.draw(device, queue, &mut encoder, &target, format, [size.0, size.1]);
        buffers.push(encoder.finish());
        buffers
    }

    /// The back buffer scaled to `size` by MetalFX, when asked for and
    /// possible now: its commands, and the image to show (1:1).
    #[cfg(target_vendor = "apple")]
    fn upscale(
        &mut self,
        device: &wgpu::Device,
        src: &Present,
        size: (u32, u32),
    ) -> Option<(Vec<wgpu::CommandBuffer>, wgpu::TextureView)> {
        let wanted = self.presentation.upscaler() == Upscaler::MetalFx && size.0 > src.width;
        self.metalfx.upscale(device, src, size, wanted)
    }

    #[cfg(not(target_vendor = "apple"))]
    fn upscale(
        &mut self,
        _: &wgpu::Device,
        _: &Present,
        _: (u32, u32),
    ) -> Option<(Vec<wgpu::CommandBuffer>, wgpu::TextureView)> {
        let _ = self.presentation.upscaler();
        None
    }
}

impl Gpu {
    pub fn present(&mut self, id: u32) {
        self.flush_clears();
        self.end_pass();
        let present = if self.window { self.present_view(id) } else { None };
        if self.settings.shot_every > 0
            && (self.stats.presents + self.total_presents).is_multiple_of(self.settings.shot_every as u64)
        {
            let name = format!("frame{:06}.png", self.stats.presents + self.total_presents);
            self.screenshot(id, &name);
        }
        self.flush_with("present", present, true);
        let now = Instant::now();
        if let Some(last) = self.last_present.replace(now) {
            self.stats.frame_ms.push((now - last).as_secs_f32() * 1000.0);
        }
        self.uniforms.next_frame();
        self.stats.presents += 1;
        if self.last_stats.elapsed() >= Duration::from_secs(10) {
            self.total_presents += self.stats.presents;
            self.log_stats();
        }
        if self.last_save.elapsed() >= Duration::from_secs(60) {
            self.save_caches();
        }
    }

    /// The back buffer, for the submit thread to scale into the window.
    fn present_view(&mut self, id: u32) -> Option<crate::submit::Present> {
        let epoch = self.epoch;
        let t = self.objs.tex_mut(id)?;
        let view = t.present.get_or_insert_with(|| {
            t.tex.create_view(&wgpu::TextureViewDescriptor {
                // sRGB back buffers hold encoded values: read them raw.
                format: Some(t.format.remove_srgb_suffix()),
                dimension: Some(wgpu::TextureViewDimension::D2),
                mip_level_count: Some(1),
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
        let view = view.clone();
        t.used.set(epoch);
        Some(crate::submit::Present { view, texture: t.tex.clone(), width: t.width, height: t.height })
    }

    /// Writes texture `id` to a PNG in the screenshot directory (once the GPU is done).
    pub fn screenshot(&mut self, id: u32, name: &str) {
        let Some(dir) = self.settings.shot_dir.clone() else {
            crate::log_once("noshotdir", || "screenshot requested without a screenshot directory".into());
            return;
        };
        let file = Path::new(name).file_name().map_or_else(|| "shot.png".into(), |f| f.to_owned());
        let path = dir.join(file).with_extension("png");
        let epoch = self.epoch;
        let Some(t) = self.objs.tex(id) else { return };
        if t.samples > 1 || t.format.block_copy_size(None) != Some(4) {
            crate::log_once(&format!("shot{:?}", t.format), || format!("screenshot of {:?} not supported", t.format));
            return;
        }
        let (w, h, format, tex) = (t.width, t.height, t.format, t.tex.clone());
        t.used.set(epoch);
        let bpr = (w * 4).next_multiple_of(256);
        let staging = self.staging.get(&self.device, (bpr * h) as u64);
        self.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: Default::default(),
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.maps.push((
            staging,
            Box::new(move |s| Work::Screenshot { staging: s, width: w, height: h, bpr, format, path }),
        ));
    }

    /// Pipeline recipes and the driver's pipeline cache, for the next run.
    pub fn save_caches(&mut self) {
        self.last_save = Instant::now();
        let Some(dir) = self.settings.cache_dir.clone() else {
            return;
        };
        if !self.recipes_new.is_empty() {
            use std::io::Write;
            let lines: String = self.recipes_new.drain(..).map(|r| r.to_json().to_string() + "\n").collect();
            let path = dir.join("pipelines.jsonl");
            let result = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| f.write_all(lines.as_bytes()));
            if let Err(e) = result {
                log::warn!("gpu: saving {}: {e}", path.display());
            }
        }
        if let Some(cache) = &self.compiler.cache
            && let Some(data) = cache.get_data()
        {
            let path = dir.join(crate::pipeline_cache_name(&self.adapter.get_info()));
            if let Err(e) = std::fs::write(&path, data) {
                log::warn!("gpu: saving {}: {e}", path.display());
            }
        }
    }
}

/// An 8-bit RGBA/BGRA image to PNG.
pub fn write_png(path: &Path, data: &[u8], width: u32, height: u32, bpr: u32, format: TextureFormat) -> Result<()> {
    let bgra = match format.remove_srgb_suffix() {
        TextureFormat::Bgra8Unorm => true,
        TextureFormat::Rgba8Unorm => false,
        other => bail!("{other:?} is not an 8-bit colour format"),
    };
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let line = &data[(row * bpr) as usize..(row * bpr + width * 4) as usize];
        for px in line.as_chunks::<4>().0 {
            let (r, b) = if bgra { (px[2], px[0]) } else { (px[0], px[2]) };
            rgba.extend_from_slice(&[r, px[1], b, 255]);
        }
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn present_shader_validates() {
        let module = naga::front::wgsl::parse_str(super::PRESENT_WGSL).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap();
    }
}
