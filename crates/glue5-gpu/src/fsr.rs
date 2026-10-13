//! AMD's FSR 3 upscaler (temporal), on any GPU, through wgpu-ffx: the same
//! inputs as MetalFX's temporal scaler (the scene before the HUD, its depth
//! and motion, jittered; see scene.rs), on the submit thread before the
//! present pass, which puts the HUD back over its image. Its compute
//! passes are SPIR-V, which wgpu translates where the backend is not Vulkan.

use std::time::Instant;

use wgpu_ffx::{FsrContext, FsrContextFlags, FsrContextInfo, FsrDispatchFlags, FsrDispatchInfo, FsrView};

use crate::present::{Scaled, Upscaled};
use crate::submit::Present;

/// The images FSR writes and keeps, for one render and one output size.
struct Sized {
    render: (u32, u32),
    output: (u32, u32),
    view: FsrView,
    dilated_depth: wgpu::Texture,
    dilated_motion: wgpu::Texture,
    previous_depth: wgpu::Buffer,
    image: wgpu::Texture,
    image_view: wgpu::TextureView,
}

#[derive(Default)]
pub struct Fsr {
    /// Made when first used (it compiles its passes), for the depth
    /// direction it was made with.
    context: Option<(FsrContext, bool)>,
    /// The context could not be made: bicubic instead.
    failed: bool,
    sized: Option<Sized>,
    /// The sizes last asked for, and since when (see `SETTLE`).
    wanted: Option<((u32, u32, u32, u32), Instant)>,
}

/// Sizes hold this long before FSR's images are made for them.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

impl Fsr {
    /// FSR's memory back while it is not used.
    pub fn release(&mut self) {
        self.sized = None;
        self.wanted = None;
    }

    /// The frame's scene upscaled to `size` (sharpened by `sharpness`, 0 to
    /// 1), when it has one and FSR works here.
    pub fn upscale(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &Present,
        size: (u32, u32),
        sharpness: f32,
        dt: f32,
    ) -> Option<Upscaled> {
        let scene = src.scene.as_ref()?;
        if self.failed {
            return None;
        }
        let reversed = scene.camera.reversed;
        if self.context.as_ref().is_none_or(|(_, r)| *r != reversed) {
            let mut flags = FsrContextFlags::empty();
            if reversed {
                flags |= FsrContextFlags::DEPTH_INVERTED;
            }
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let context = FsrContext::new(FsrContextInfo { device: device.clone(), flags });
            if let Some(e) = pollster::block_on(scope.pop()) {
                log::warn!("gpu: FSR does not work on this GPU, bicubic instead: {e}");
                self.failed = true;
                return None;
            }
            log::info!("gpu: FSR 3 upscaler ready");
            self.context = Some((context, reversed));
            self.sized = None;
        }
        let render = (src.width, src.height);
        let key = (render.0, render.1, size.0, size.1);
        if self.sized.as_ref().is_none_or(|s| (s.render, s.output) != (render, size)) {
            match self.wanted {
                Some((k, since)) if k == key && since.elapsed() >= SETTLE => {}
                Some((k, _)) if k == key => return None,
                _ => {
                    self.wanted = Some((key, Instant::now()));
                    return None;
                }
            }
            let (context, _) = self.context.as_ref()?;
            self.sized = Some(Sized::new(device, queue, context, render, size));
            log::info!("gpu: FSR {}x{} -> {}x{}", render.0, render.1, size.0, size.1);
        }
        let (context, _) = self.context.as_ref()?;
        let s = self.sized.as_mut()?;
        let camera = scene.camera;
        // Inverted depth: the near plane is the larger distance to FSR.
        let (near, far) = if reversed { (camera.far, camera.near) } else { (camera.near, camera.far) };
        let info = FsrDispatchInfo {
            color: scene.color.clone(),
            depth: scene.depth.clone(),
            motion_vectors: scene.motion.clone(),
            exposure: None,
            reactive_mask: None,
            transparency_and_composition: None,
            dilated_depth: s.dilated_depth.clone(),
            dilated_motion_vectors: s.dilated_motion.clone(),
            reconstructed_previous_depth: s.previous_depth.clone(),
            output: s.image.clone(),
            jitter_offset: scene.jitter,
            // Already in the scene's pixels.
            motion_vector_scale: [1.0, 1.0],
            render_size: [render.0, render.1],
            upscale_size: [size.0, size.1],
            enable_sharpening: sharpness > 0.0,
            sharpness: sharpness.clamp(0.0, 1.0),
            frame_time_delta: (dt * 1000.0).max(1.0),
            pre_exposure: 1.0,
            reset_history: scene.reset,
            camera_near: near,
            camera_far: far,
            camera_fov_y: camera.fov_y.to_radians(),
            view_space_to_meters_factor: 1.0,
            flags: FsrDispatchFlags::empty(),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fsr") });
        if let Err(e) = context.dispatch(&mut s.view, &mut encoder, &info) {
            crate::log_once("fsr dispatch", || format!("FSR: {e}"));
            return None;
        }
        Some(Upscaled { commands: vec![encoder.finish()], image: Scaled::Scene(s.image_view.clone()), between: None })
    }
}

impl Sized {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        context: &FsrContext,
        render: (u32, u32),
        output: (u32, u32),
    ) -> Sized {
        let texture = |label, (w, h): (u32, u32), format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                // (Copied out by the tests.)
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let image = texture("fsr output", output, wgpu::TextureFormat::Rgba16Float);
        Sized {
            render,
            output,
            view: context.create_view(queue, [render.0, render.1], [output.0, output.1]),
            dilated_depth: texture("fsr dilated depth", render, wgpu::TextureFormat::R32Float),
            // (Four channels: Metal cannot write two-channel half floats in
            // FSR's passes; see third_party/wgpu-ffx.)
            dilated_motion: texture("fsr dilated motion", render, wgpu::TextureFormat::Rgba16Float),
            previous_depth: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fsr previous depth"),
                size: render.0 as u64 * render.1 as u64 * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            image_view: image.create_view(&Default::default()),
            image,
        }
    }
}
