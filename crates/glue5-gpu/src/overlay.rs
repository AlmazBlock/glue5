//! What the application draws over the game — its settings and touch
//! controls — as egui primitives, rendered into the window after the game's
//! frame.

use std::sync::{Arc, Mutex};

use egui::TexturesDelta;
use egui::epaint::ClippedPrimitive;

/// Shared by the application (which submits) and the renderer (which draws
/// the latest submission at each present).
#[derive(Clone, Default)]
pub struct Overlay(Arc<Mutex<Frame>>);

#[derive(Default)]
struct Frame {
    primitives: Arc<Vec<ClippedPrimitive>>,
    /// Texture changes accumulate until a present applies them.
    textures: TexturesDelta,
    pixels_per_point: f32,
}

impl Drop for Frame {
    fn drop(&mut self) {
        // Pending uploads can be discarded once the overlay is no longer used.
        self.textures.clear();
    }
}

impl Overlay {
    /// Replaces what is drawn (an empty list draws nothing).
    pub fn submit(&self, primitives: Vec<ClippedPrimitive>, textures: TexturesDelta, pixels_per_point: f32) {
        let mut frame = self.0.lock().unwrap();
        frame.primitives = Arc::new(primitives);
        frame.textures.append(textures);
        frame.pixels_per_point = pixels_per_point;
    }

    fn take(&self) -> (Arc<Vec<ClippedPrimitive>>, TexturesDelta, f32) {
        let mut frame = self.0.lock().unwrap();
        (frame.primitives.clone(), std::mem::take(&mut frame.textures), frame.pixels_per_point)
    }
}

/// The renderer side.
pub struct OverlayRenderer {
    overlay: Overlay,
    renderer: Option<(wgpu::TextureFormat, egui_wgpu::Renderer)>,
}

impl OverlayRenderer {
    pub fn new(overlay: Overlay) -> Self {
        OverlayRenderer { overlay, renderer: None }
    }

    /// Draws the overlay into `target` (already holding the game's frame).
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: [u32; 2],
    ) {
        let (primitives, mut textures, pixels_per_point) = self.overlay.take();
        if primitives.is_empty() && textures.is_empty() {
            return;
        }
        if self.renderer.as_ref().is_none_or(|(f, _)| *f != format) {
            self.renderer = Some((format, egui_wgpu::Renderer::new(device, format, Default::default())));
        }
        let renderer = &mut self.renderer.as_mut().unwrap().1;
        for (id, deltas) in &textures.set {
            for delta in deltas {
                renderer.update_texture(device, queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point };
        if !primitives.is_empty() {
            renderer.update_buffers(device, queue, encoder, &primitives, &screen);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("overlay"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            renderer.render(&mut pass, &primitives, &screen);
        }
        for id in &textures.free {
            renderer.free_texture(id);
        }
        textures.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_before_the_first_present_discards_pending_uploads() {
        let overlay = Overlay::default();
        let mut textures = TexturesDelta::default();
        textures.push(
            egui::TextureId::Managed(0),
            egui::epaint::ImageDelta::full(
                egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                egui::TextureOptions::LINEAR,
            ),
        );
        overlay.submit(Vec::new(), textures, 1.0);
        // TexturesDelta asserts on drop unless consumed or explicitly cleared.
        drop(overlay);
    }
}
