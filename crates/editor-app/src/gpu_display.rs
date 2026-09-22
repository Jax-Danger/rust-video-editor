//! Program-monitor display upload.
//!
//! [`editor_media::compose_layers`](editor_media::compose_layers) still produces the
//! RGBA that Deliver encodes. This module only copies that buffer to the GPU so
//! the program monitor can sample one texture while scrubbing or playing.
//!
//! Backend order:
//! 1. **wgpu** — `queue.write_texture` into `Rgba8UnormSrgb`, when the app is
//!    built with `--features wgpu` and that renderer is current.
//! 2. **glow** — `texSubImage2D` into one `RGBA8` texture. This is the default
//!    eframe renderer.
//! 3. **CPU** — one reused egui [`ColorImage`] texture, when neither context
//!    exists (headless tests, or a process whose GL/wgpu device did not come up).
//!
//! The same composite signature is not uploaded again. A new frame at the same
//! canvas size replaces the pixels in place instead of allocating a texture.

#[cfg(any(test, feature = "wgpu"))]
use std::borrow::Cow;

use egui::{ColorImage, Context, TextureHandle, TextureOptions};

/// wgpu copies rows in multiples of 256 bytes. Tests lock this without linking wgpu.
#[cfg(any(test, feature = "wgpu"))]
pub const WGPU_ROW_ALIGNMENT: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayBackend {
    Wgpu,
    Glow,
    Cpu,
}

impl DisplayBackend {
    /// Short label for the program-monitor header.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Wgpu | Self::Glow => "GPU",
            Self::Cpu => "CPU",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadPlan {
    /// Resident texture already holds this composite.
    Unchanged,
    /// Same dimensions; replace the texels.
    ReplacePixels,
    /// First upload, a backend switch, or a canvas-size change.
    Allocate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resident {
    backend: DisplayBackend,
    signature: u64,
    width: u32,
    height: u32,
    texture: egui::TextureId,
}

/// Picks the fastest display path that is actually available this frame.
pub fn choose_backend(wgpu_ready: bool, glow_ready: bool) -> DisplayBackend {
    if wgpu_ready {
        DisplayBackend::Wgpu
    } else if glow_ready {
        DisplayBackend::Glow
    } else {
        DisplayBackend::Cpu
    }
}

pub fn rgba_len_ok(width: u32, height: u32, len: usize) -> bool {
    let Some(pixels) = (width as usize).checked_mul(height as usize) else {
        return false;
    };
    let Some(bytes) = pixels.checked_mul(4) else {
        return false;
    };
    bytes > 0 && len == bytes
}

pub fn plan_upload(
    resident: Option<(DisplayBackend, u64, u32, u32)>,
    backend: DisplayBackend,
    signature: u64,
    width: u32,
    height: u32,
) -> UploadPlan {
    match resident {
        Some((current, sig, w, h))
            if current == backend && sig == signature && w == width && h == height =>
        {
            UploadPlan::Unchanged
        }
        Some((current, _, w, h)) if current == backend && w == width && h == height => {
            UploadPlan::ReplacePixels
        }
        _ => UploadPlan::Allocate,
    }
}

/// Bytes in one destination row after `alignment` padding.
#[cfg(any(test, feature = "wgpu"))]
pub fn aligned_row_bytes(width: u32, alignment: u32) -> u32 {
    let raw = width.saturating_mul(4);
    let alignment = alignment.max(1);
    raw.div_ceil(alignment).saturating_mul(alignment)
}

/// Copy tightly packed RGBA into rows padded to `alignment`.
///
/// Already-aligned frames are returned borrowed so a 960-wide preview does not
/// allocate a second buffer on the wgpu path.
#[cfg(any(test, feature = "wgpu"))]
pub fn pad_rgba_rows<'a>(rgba: &'a [u8], width: u32, height: u32, alignment: u32) -> Cow<'a, [u8]> {
    let src_stride = (width as usize).saturating_mul(4);
    let dst_stride = aligned_row_bytes(width, alignment) as usize;
    if dst_stride == src_stride || height == 0 || src_stride == 0 {
        return Cow::Borrowed(rgba);
    }
    let mut out = vec![0u8; dst_stride.saturating_mul(height as usize)];
    for y in 0..height as usize {
        let src = y * src_stride;
        let dst = y * dst_stride;
        let end = src + src_stride;
        if end > rgba.len() || dst + src_stride > out.len() {
            break;
        }
        out[dst..dst + src_stride].copy_from_slice(&rgba[src..end]);
    }
    Cow::Owned(out)
}

pub struct UploadedFrame {
    pub texture: egui::TextureId,
    pub width: u32,
    pub height: u32,
}

struct GlowTex {
    native: eframe::glow::Texture,
    width: u32,
    height: u32,
    id: Option<egui::TextureId>,
}

#[cfg(feature = "wgpu")]
struct WgpuTex {
    texture: eframe::wgpu::Texture,
    // Held so egui's bind group does not outlive the texture view.
    #[allow(dead_code)]
    view: eframe::wgpu::TextureView,
    width: u32,
    height: u32,
    id: Option<egui::TextureId>,
}

#[derive(Default)]
pub struct ProgramDisplay {
    resident: Option<Resident>,
    backend: DisplayBackend,
    glow: Option<GlowTex>,
    #[cfg(feature = "wgpu")]
    wgpu_tex: Option<WgpuTex>,
    cpu: Option<TextureHandle>,
}

impl Default for DisplayBackend {
    fn default() -> Self {
        Self::Cpu
    }
}

impl ProgramDisplay {
    pub fn badge(&self) -> &'static str {
        self.backend.badge()
    }

    /// Upload `rgba` when the composite changed. Returns the egui texture to paint.
    pub fn upload(
        &mut self,
        ctx: &Context,
        host: &mut eframe::Frame,
        signature: u64,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Option<UploadedFrame> {
        if !rgba_len_ok(width, height, rgba.len()) {
            return None;
        }
        let backend = choose_backend(wgpu_ready(host), host.gl().is_some());
        let plan = plan_upload(
            self.resident
                .map(|r| (r.backend, r.signature, r.width, r.height)),
            backend,
            signature,
            width,
            height,
        );
        if plan == UploadPlan::Unchanged {
            let resident = self.resident?;
            return Some(UploadedFrame {
                texture: resident.texture,
                width,
                height,
            });
        }

        let texture = match backend {
            DisplayBackend::Wgpu => self.upload_wgpu(host, width, height, rgba),
            DisplayBackend::Glow => self.upload_glow(host, width, height, rgba),
            DisplayBackend::Cpu => None,
        };
        let (backend, texture) = match texture {
            Some(texture) => (backend, texture),
            None => (
                DisplayBackend::Cpu,
                self.upload_cpu(ctx, width, height, rgba)?,
            ),
        };

        self.backend = backend;
        self.resident = Some(Resident {
            backend,
            signature,
            width,
            height,
            texture,
        });
        Some(UploadedFrame {
            texture,
            width,
            height,
        })
    }

    fn upload_cpu(
        &mut self,
        ctx: &Context,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Option<egui::TextureId> {
        let image = ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba);
        if let Some(handle) = &mut self.cpu {
            handle.set(image, TextureOptions::LINEAR);
            return Some(handle.id());
        }
        let handle = ctx.load_texture("meridian-program", image, TextureOptions::LINEAR);
        let id = handle.id();
        self.cpu = Some(handle);
        Some(id)
    }

    fn upload_glow(
        &mut self,
        host: &mut eframe::Frame,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Option<egui::TextureId> {
        use eframe::glow::HasContext as _;

        let gl = host.gl().cloned()?;
        let (Some(gl_w), Some(gl_h)) = (i32::try_from(width).ok(), i32::try_from(height).ok())
        else {
            return None;
        };
        // Safety: eframe makes the glow context current for `App::update`.
        // These calls are the context's texture entry points. `rgba` is packed
        // RGBA8 with length width*height*4, UNPACK_ALIGNMENT is 1 for the
        // upload, and the texture name is one this function created. Alignment
        // is restored and the texture unbound before we return.
        let native = unsafe {
            if self.glow.is_none() {
                let native = gl.create_texture().ok()?;
                self.glow = Some(GlowTex {
                    native,
                    width: 0,
                    height: 0,
                    id: None,
                });
            }
            let slot = self.glow.as_mut()?;
            let realloc = slot.width != width || slot.height != height || slot.id.is_none();
            let native = slot.native;
            gl.bind_texture(eframe::glow::TEXTURE_2D, Some(native));
            gl.pixel_store_i32(eframe::glow::UNPACK_ALIGNMENT, 1);
            if realloc {
                gl.tex_image_2d(
                    eframe::glow::TEXTURE_2D,
                    0,
                    eframe::glow::RGBA8 as i32,
                    gl_w,
                    gl_h,
                    0,
                    eframe::glow::RGBA,
                    eframe::glow::UNSIGNED_BYTE,
                    eframe::glow::PixelUnpackData::Slice(Some(rgba)),
                );
                gl.tex_parameter_i32(
                    eframe::glow::TEXTURE_2D,
                    eframe::glow::TEXTURE_MIN_FILTER,
                    eframe::glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    eframe::glow::TEXTURE_2D,
                    eframe::glow::TEXTURE_MAG_FILTER,
                    eframe::glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    eframe::glow::TEXTURE_2D,
                    eframe::glow::TEXTURE_WRAP_S,
                    eframe::glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    eframe::glow::TEXTURE_2D,
                    eframe::glow::TEXTURE_WRAP_T,
                    eframe::glow::CLAMP_TO_EDGE as i32,
                );
            } else {
                gl.tex_sub_image_2d(
                    eframe::glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    gl_w,
                    gl_h,
                    eframe::glow::RGBA,
                    eframe::glow::UNSIGNED_BYTE,
                    eframe::glow::PixelUnpackData::Slice(Some(rgba)),
                );
            }
            gl.pixel_store_i32(eframe::glow::UNPACK_ALIGNMENT, 4);
            gl.bind_texture(eframe::glow::TEXTURE_2D, None);
            native
        };

        let needs_id = {
            let slot = self.glow.as_mut()?;
            slot.width = width;
            slot.height = height;
            slot.id.is_none()
        };
        if needs_id {
            let id = host.register_native_glow_texture(native);
            self.glow.as_mut()?.id = Some(id);
        }
        self.glow.as_ref()?.id
    }

    #[cfg(feature = "wgpu")]
    fn upload_wgpu(
        &mut self,
        host: &mut eframe::Frame,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Option<egui::TextureId> {
        let state = host.wgpu_render_state()?.clone();
        let realloc = self
            .wgpu_tex
            .as_ref()
            .is_none_or(|tex| tex.width != width || tex.height != height || tex.id.is_none());
        if realloc {
            let texture = state
                .device
                .create_texture(&eframe::wgpu::TextureDescriptor {
                    label: Some("meridian-program"),
                    size: eframe::wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: eframe::wgpu::TextureDimension::D2,
                    format: eframe::wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: eframe::wgpu::TextureUsages::TEXTURE_BINDING
                        | eframe::wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
            let view = texture.create_view(&eframe::wgpu::TextureViewDescriptor::default());
            let id = {
                let mut renderer = state.renderer.write();
                if let Some(existing) = self.wgpu_tex.as_ref().and_then(|tex| tex.id) {
                    renderer.update_egui_texture_from_wgpu_texture(
                        &state.device,
                        &view,
                        eframe::wgpu::FilterMode::Linear,
                        existing,
                    );
                    existing
                } else {
                    renderer.register_native_texture(
                        &state.device,
                        &view,
                        eframe::wgpu::FilterMode::Linear,
                    )
                }
            };
            self.wgpu_tex = Some(WgpuTex {
                texture,
                view,
                width,
                height,
                id: Some(id),
            });
        }

        debug_assert_eq!(
            eframe::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
            WGPU_ROW_ALIGNMENT
        );
        let bytes_per_row = aligned_row_bytes(width, eframe::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let padded = pad_rgba_rows(
            rgba,
            width,
            height,
            eframe::wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let tex = &self.wgpu_tex.as_ref()?.texture;
        state.queue.write_texture(
            eframe::wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: eframe::wgpu::Origin3d::ZERO,
                aspect: eframe::wgpu::TextureAspect::All,
            },
            padded.as_ref(),
            eframe::wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            eframe::wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.wgpu_tex.as_ref()?.id
    }

    #[cfg(not(feature = "wgpu"))]
    fn upload_wgpu(
        &mut self,
        _host: &mut eframe::Frame,
        _width: u32,
        _height: u32,
        _rgba: &[u8],
    ) -> Option<egui::TextureId> {
        None
    }
}

fn wgpu_ready(host: &eframe::Frame) -> bool {
    #[cfg(feature = "wgpu")]
    {
        host.wgpu_render_state().is_some()
    }
    #[cfg(not(feature = "wgpu"))]
    {
        let _ = host;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_prefers_wgpu_then_glow_then_cpu() {
        assert_eq!(choose_backend(true, true), DisplayBackend::Wgpu);
        assert_eq!(choose_backend(true, false), DisplayBackend::Wgpu);
        assert_eq!(choose_backend(false, true), DisplayBackend::Glow);
        assert_eq!(choose_backend(false, false), DisplayBackend::Cpu);
        assert_eq!(DisplayBackend::Glow.badge(), "GPU");
        assert_eq!(DisplayBackend::Wgpu.badge(), "GPU");
        assert_eq!(DisplayBackend::Cpu.badge(), "CPU");
    }

    #[test]
    fn playback_replaces_pixels_and_a_repeat_frame_does_not() {
        let resident = Some((DisplayBackend::Glow, 10, 960, 540));
        assert_eq!(
            plan_upload(resident, DisplayBackend::Glow, 10, 960, 540),
            UploadPlan::Unchanged
        );
        assert_eq!(
            plan_upload(resident, DisplayBackend::Glow, 11, 960, 540),
            UploadPlan::ReplacePixels
        );
        assert_eq!(
            plan_upload(resident, DisplayBackend::Glow, 11, 304, 540),
            UploadPlan::Allocate
        );
        assert_eq!(
            plan_upload(resident, DisplayBackend::Cpu, 10, 960, 540),
            UploadPlan::Allocate
        );
        assert_eq!(
            plan_upload(None, DisplayBackend::Glow, 1, 960, 540),
            UploadPlan::Allocate
        );
    }

    #[test]
    fn rgba_length_matches_the_canvas() {
        assert!(rgba_len_ok(2, 2, 16));
        assert!(!rgba_len_ok(2, 2, 15));
        assert!(!rgba_len_ok(0, 2, 0));
        assert!(!rgba_len_ok(u32::MAX, u32::MAX, usize::MAX));
    }

    #[test]
    fn wgpu_rows_pad_to_256_and_keep_pixels() {
        assert_eq!(aligned_row_bytes(960, WGPU_ROW_ALIGNMENT), 960 * 4);
        let tight = vec![7u8, 8, 9, 10, 11, 12, 13, 14];
        let padded = pad_rgba_rows(&tight, 2, 1, WGPU_ROW_ALIGNMENT);
        assert!(matches!(padded, Cow::Owned(_)));
        assert_eq!(padded.len(), 256);
        assert_eq!(&padded[..8], &tight[..]);
        assert!(padded[8..].iter().all(|byte| *byte == 0));

        let wide = vec![1u8; (64 * 4) as usize];
        let borrowed = pad_rgba_rows(&wide, 64, 1, WGPU_ROW_ALIGNMENT);
        assert!(matches!(borrowed, Cow::Borrowed(_)));
        assert_eq!(borrowed.as_ref(), wide.as_slice());

        let rows = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let padded = pad_rgba_rows(&rows, 2, 2, 16);
        assert_eq!(padded.len(), 32);
        assert_eq!(&padded[0..8], &rows[0..8]);
        assert_eq!(&padded[16..24], &rows[8..16]);
    }
}
