//! Shared picture composite for the program monitor and Deliver.
//!
//! Preview (`--features ffmpeg`) and export both evaluate this module. The
//! grade is one formula — exposure in stops, contrast about mid grey, a split
//! shadow/highlight lift, temperature, tint, then luma saturation. It runs on
//! the decoded RGB values as stored (no scene-linear conversion). Transforms
//! are position, scale, rotation, anchor, and opacity, then a straight alpha
//! over. Dissolves, wipes, and pushes are the same `transition_motion` in both
//! paths.
//!
//! The raster size may differ (the monitor fits inside 960×540, export uses the
//! sequence size, capped at the preview decoder's 1920-pixel edge). The mapping
//! is the same, so the pictures agree up to that scale and the codec. Clip
//! speed chooses the source frame in `source_frame_at` before either path
//! decodes, so a ramp or a constant rate is the same picture in both.

use editor_core::{
    blur, chroma_key, clip_relative, color_grade, crop, picture_at, sharpen, source_frame_at,
    transform, vignette, Clip, ColorGrade, Direction, Frame, MediaAsset, MulticamGroup, Sequence,
    SequenceId, Title, ToneCurve, Track, TrackKind, Transform, TransitionKind, MAX_NEST_DEPTH,
};
use font8x8::UnicodeFonts;

#[derive(Clone, Debug, PartialEq)]
pub struct GradeSample {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub temperature: f32,
    pub tint: f32,
    pub saturation: f32,
    pub lift: [f32; 3],
    pub gamma: [f32; 3],
    pub gain: [f32; 3],
    pub luma_curve: ToneCurve,
}

impl GradeSample {
    pub fn neutral() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            highlights: 0.0,
            shadows: 0.0,
            temperature: 0.0,
            tint: 0.0,
            saturation: 1.0,
            lift: [0.0, 0.0, 0.0],
            gamma: [0.0, 0.0, 0.0],
            gain: [0.0, 0.0, 0.0],
            luma_curve: ToneCurve::identity(),
        }
    }

    pub fn from_grade(grade: &ColorGrade, rel: i64) -> Self {
        Self {
            exposure: grade.exposure.value_at(rel),
            contrast: grade.contrast.value_at(rel),
            highlights: grade.highlights.value_at(rel),
            shadows: grade.shadows.value_at(rel),
            temperature: grade.temperature.value_at(rel),
            tint: grade.tint.value_at(rel),
            saturation: grade.saturation.value_at(rel),
            lift: grade.lift.values_at(rel),
            gamma: grade.gamma.values_at(rel),
            gain: grade.gain.values_at(rel),
            luma_curve: grade.luma_curve.clone(),
        }
    }

    pub fn from_effects(effects: &[editor_core::Effect], rel: i64) -> Self {
        color_grade(effects)
            .map(|grade| Self::from_grade(grade, rel))
            .unwrap_or_else(Self::neutral)
    }

    pub fn is_neutral(&self) -> bool {
        self.exposure.abs() < 1.0e-4
            && (self.contrast - 1.0).abs() < 1.0e-4
            && self.highlights.abs() < 1.0e-4
            && self.shadows.abs() < 1.0e-4
            && self.temperature.abs() < 1.0e-4
            && self.tint.abs() < 1.0e-4
            && (self.saturation - 1.0).abs() < 1.0e-4
            && self.lift.iter().all(|v| v.abs() < 1.0e-4)
            && self.gamma.iter().all(|v| v.abs() < 1.0e-4)
            && self.gain.iter().all(|v| v.abs() < 1.0e-4)
            && self.luma_curve == ToneCurve::identity()
    }

    fn apply_luma_curve(rgb: [f32; 3], curve: &ToneCurve) -> [f32; 3] {
        if curve == &ToneCurve::identity() {
            return rgb;
        }
        let luma = rec709_luma(rgb);
        let mapped = curve.eval(luma);
        if luma < 1.0e-4 {
            return rgb.map(|_| mapped.clamp(0.0, 1.5));
        }
        let scale = mapped / luma;
        rgb.map(|channel| (channel * scale).clamp(0.0, 1.5))
    }

    fn apply_wheel_offsets(
        rgb: [f32; 3],
        lift: [f32; 3],
        gamma: [f32; 3],
        gain: [f32; 3],
    ) -> [f32; 3] {
        let luma = rec709_luma(rgb);
        let lift_w = 1.0 - smoothstep(0.08, 0.40, luma);
        let gamma_w = smoothstep(0.20, 0.45, luma) * (1.0 - smoothstep(0.55, 0.80, luma));
        let gain_w = smoothstep(0.60, 0.95, luma);
        let strength = 0.35;
        [
            (rgb[0]
                + lift[0] * strength * lift_w
                + gamma[0] * strength * gamma_w
                + gain[0] * strength * gain_w)
                .clamp(0.0, 1.5),
            (rgb[1]
                + lift[1] * strength * lift_w
                + gamma[1] * strength * gamma_w
                + gain[1] * strength * gain_w)
                .clamp(0.0, 1.5),
            (rgb[2]
                + lift[2] * strength * lift_w
                + gamma[2] * strength * gamma_w
                + gain[2] * strength * gain_w)
                .clamp(0.0, 1.5),
        ]
    }

    /// Shared grade. `rgb` channels are 0…1 display values.
    ///
    /// Shadows and highlights use a smooth split: shadows fall off by mid grey,
    /// highlights start there, so a shadow lift does not also brighten the
    /// whites. Coefficients are 0.45 of the parameter at full weight.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut c = Self::apply_luma_curve(rgb, &self.luma_curve);
        c = c.map(|channel| channel * 2.0_f32.powf(self.exposure));
        c = c.map(|channel| ((channel - 0.5) * self.contrast + 0.5).clamp(0.0, 1.5));
        c = Self::apply_wheel_offsets(c, self.lift, self.gamma, self.gain);
        c = c.map(|channel| {
            let shadow_w = 1.0 - smoothstep(0.10, 0.55, channel);
            let high_w = smoothstep(0.45, 0.90, channel);
            (channel + self.shadows * 0.45 * shadow_w + self.highlights * 0.45 * high_w)
                .clamp(0.0, 1.5)
        });
        c[0] = (c[0] + self.temperature * 0.18).clamp(0.0, 1.5);
        c[2] = (c[2] - self.temperature * 0.18).clamp(0.0, 1.5);
        c[1] = (c[1] - self.tint * 0.14).clamp(0.0, 1.5);
        c[0] = (c[0] + self.tint * 0.06).clamp(0.0, 1.5);
        c[2] = (c[2] + self.tint * 0.06).clamp(0.0, 1.5);
        let luma = rec709_luma(c);
        c.map(|channel| (luma + (channel - luma) * self.saturation).clamp(0.0, 1.0))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterSample {
    pub blur_radius: f32,
    pub vignette_amount: f32,
    pub vignette_softness: f32,
    pub crop: [f32; 4],
    pub sharpen: f32,
    pub chroma_key_color: [f32; 3],
    pub chroma_key_tolerance: f32,
    pub chroma_key_softness: f32,
    pub chroma_key_spill: f32,
}

impl FilterSample {
    pub fn neutral() -> Self {
        Self {
            blur_radius: 0.0,
            vignette_amount: 0.0,
            vignette_softness: 0.5,
            crop: [0.0, 0.0, 0.0, 0.0],
            sharpen: 0.0,
            chroma_key_color: [0.0, 1.0, 0.0],
            chroma_key_tolerance: 0.0,
            chroma_key_softness: 0.15,
            chroma_key_spill: 0.0,
        }
    }

    pub fn from_effects(effects: &[editor_core::Effect], rel: i64) -> Self {
        Self {
            blur_radius: blur(effects)
                .map(|f| f.radius.value_at(rel))
                .unwrap_or(0.0)
                .max(0.0),
            vignette_amount: vignette(effects)
                .map(|f| f.amount.value_at(rel))
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
            vignette_softness: vignette(effects)
                .map(|f| f.softness.value_at(rel))
                .unwrap_or(0.5)
                .clamp(0.05, 1.0),
            crop: {
                let c = crop(effects);
                [
                    c.map(|f| f.left.value_at(rel))
                        .unwrap_or(0.0)
                        .clamp(0.0, 0.45),
                    c.map(|f| f.right.value_at(rel))
                        .unwrap_or(0.0)
                        .clamp(0.0, 0.45),
                    c.map(|f| f.top.value_at(rel))
                        .unwrap_or(0.0)
                        .clamp(0.0, 0.45),
                    c.map(|f| f.bottom.value_at(rel))
                        .unwrap_or(0.0)
                        .clamp(0.0, 0.45),
                ]
            },
            sharpen: sharpen(effects)
                .map(|f| f.amount.value_at(rel))
                .unwrap_or(0.0)
                .clamp(0.0, 2.0),
            chroma_key_color: {
                let ck = chroma_key(effects);
                [
                    ck.map(|f| f.key_red.value_at(rel))
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0),
                    ck.map(|f| f.key_green.value_at(rel))
                        .unwrap_or(1.0)
                        .clamp(0.0, 1.0),
                    ck.map(|f| f.key_blue.value_at(rel))
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0),
                ]
            },
            chroma_key_tolerance: chroma_key(effects)
                .map(|f| f.tolerance.value_at(rel))
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
            chroma_key_softness: chroma_key(effects)
                .map(|f| f.softness.value_at(rel))
                .unwrap_or(0.15)
                .clamp(0.0, 1.0),
            chroma_key_spill: chroma_key(effects)
                .map(|f| f.spill_suppression.value_at(rel))
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
        }
    }

    pub fn is_neutral(&self) -> bool {
        self.blur_radius < 0.5
            && self.vignette_amount < 1.0e-4
            && self.crop.iter().all(|v| *v < 1.0e-4)
            && self.sharpen < 1.0e-4
            && self.chroma_key_tolerance < 1.0e-4
    }
}

fn rec709_luma(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How a transition moves one side. `opacity_scale` multiplies the clip opacity.
/// Shifts are sequence pixels (+x right, +y up). The mask is in canvas space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionMotion {
    pub opacity_scale: f32,
    pub shift_x: f32,
    pub shift_y: f32,
    pub mask: CanvasMask,
    /// Extra blur radius in sequence pixels (blur dissolve).
    pub blur_radius: f32,
    /// Solid plate colour and opacity for dip-to-white (and similar).
    pub dip_plate: Option<([f32; 3], f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CanvasMask {
    None,
    /// `angle_deg` is the direction the reveal travels, clockwise from +X
    /// (0 = left to right, 90 = top to bottom). `edge` is the progress.
    /// `keep_below` keeps the side already revealed (`t <= edge`).
    Wipe {
        angle_deg: f32,
        edge: f32,
        keep_below: bool,
    },
    /// Circular iris. `edge` is the reveal radius from the centre (0…1).
    Iris {
        edge: f32,
        keep_below: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaskWindow {
    All,
    Empty,
    /// Visible canvas window, normalized, x right and y down.
    Uv {
        u0: f32,
        v0: f32,
        u1: f32,
        v1: f32,
    },
    /// Not an axis-aligned wipe. Test [`mask_allows`] per pixel.
    PerPixel,
}

pub fn transition_motion(
    kind: &TransitionKind,
    progress: f32,
    outgoing: bool,
    seq_w: f32,
    seq_h: f32,
) -> TransitionMotion {
    let p = progress.clamp(0.0, 1.0);
    match kind {
        TransitionKind::CrossDissolve => TransitionMotion {
            // Incoming dissolves over a fully opaque outgoing plate, which is a
            // linear mix when the outgoing clip itself is opaque.
            opacity_scale: if outgoing { 1.0 } else { p },
            shift_x: 0.0,
            shift_y: 0.0,
            mask: CanvasMask::None,
            blur_radius: 0.0,
            dip_plate: None,
        },
        TransitionKind::Wipe { angle_deg } => TransitionMotion {
            opacity_scale: 1.0,
            shift_x: 0.0,
            shift_y: 0.0,
            mask: CanvasMask::Wipe {
                angle_deg: *angle_deg,
                edge: p,
                keep_below: !outgoing,
            },
            blur_radius: 0.0,
            dip_plate: None,
        },
        TransitionKind::PushSlide { direction } => {
            let (shift_x, shift_y) = push_shift(*direction, outgoing, p, seq_w, seq_h);
            TransitionMotion {
                opacity_scale: 1.0,
                shift_x,
                shift_y,
                mask: CanvasMask::None,
                blur_radius: 0.0,
                dip_plate: None,
            }
        }
        TransitionKind::DipToBlack => TransitionMotion {
            opacity_scale: if outgoing {
                if p <= 0.5 {
                    1.0 - 2.0 * p
                } else {
                    0.0
                }
            } else if p <= 0.5 {
                0.0
            } else {
                2.0 * p - 1.0
            },
            shift_x: 0.0,
            shift_y: 0.0,
            mask: CanvasMask::None,
            blur_radius: 0.0,
            dip_plate: None,
        },
        TransitionKind::DipToWhite => {
            let plate_opacity = if p <= 0.5 { 2.0 * p } else { 2.0 * (1.0 - p) };
            TransitionMotion {
                opacity_scale: if outgoing {
                    if p <= 0.5 {
                        1.0 - 2.0 * p
                    } else {
                        0.0
                    }
                } else if p <= 0.5 {
                    0.0
                } else {
                    2.0 * p - 1.0
                },
                shift_x: 0.0,
                shift_y: 0.0,
                mask: CanvasMask::None,
                blur_radius: 0.0,
                dip_plate: Some(([1.0, 1.0, 1.0], plate_opacity)),
            }
        }
        TransitionKind::Slide { direction } => {
            let (shift_x, shift_y) = if outgoing {
                (0.0, 0.0)
            } else {
                slide_shift(*direction, p, seq_w, seq_h)
            };
            TransitionMotion {
                opacity_scale: 1.0,
                shift_x,
                shift_y,
                mask: CanvasMask::None,
                blur_radius: 0.0,
                dip_plate: None,
            }
        }
        TransitionKind::BlurDissolve => {
            let peak = 24.0;
            let blur = if outgoing {
                peak * (1.0 - (2.0 * p - 1.0).abs())
            } else {
                peak * (1.0 - (2.0 * p - 1.0).abs())
            };
            TransitionMotion {
                opacity_scale: if outgoing { 1.0 } else { p },
                shift_x: 0.0,
                shift_y: 0.0,
                mask: CanvasMask::None,
                blur_radius: blur,
                dip_plate: None,
            }
        }
        TransitionKind::Iris => TransitionMotion {
            opacity_scale: 1.0,
            shift_x: 0.0,
            shift_y: 0.0,
            mask: CanvasMask::Iris {
                edge: p,
                keep_below: !outgoing,
            },
            blur_radius: 0.0,
            dip_plate: None,
        },
    }
}

fn slide_shift(direction: Direction, p: f32, seq_w: f32, seq_h: f32) -> (f32, f32) {
    match direction {
        Direction::Left => (seq_w * (1.0 - p), 0.0),
        Direction::Right => (-seq_w * (1.0 - p), 0.0),
        Direction::Up => (0.0, -seq_h * (1.0 - p)),
        Direction::Down => (0.0, seq_h * (1.0 - p)),
    }
}

fn push_shift(direction: Direction, outgoing: bool, p: f32, seq_w: f32, seq_h: f32) -> (f32, f32) {
    match direction {
        Direction::Left => {
            if outgoing {
                (-seq_w * p, 0.0)
            } else {
                (seq_w * (1.0 - p), 0.0)
            }
        }
        Direction::Right => {
            if outgoing {
                (seq_w * p, 0.0)
            } else {
                (-seq_w * (1.0 - p), 0.0)
            }
        }
        Direction::Up => {
            if outgoing {
                (0.0, seq_h * p)
            } else {
                (0.0, -seq_h * (1.0 - p))
            }
        }
        Direction::Down => {
            if outgoing {
                (0.0, -seq_h * p)
            } else {
                (0.0, seq_h * (1.0 - p))
            }
        }
    }
}

pub fn apply_transition(
    mut place: Place,
    kind: &TransitionKind,
    progress: f32,
    outgoing: bool,
    seq_w: f32,
    seq_h: f32,
) -> (Place, TransitionMotion) {
    let motion = transition_motion(kind, progress, outgoing, seq_w, seq_h);
    place.opacity = (place.opacity * motion.opacity_scale).clamp(0.0, 1.0);
    place.shift_x += motion.shift_x;
    place.shift_y += motion.shift_y;
    place.mask = motion.mask;
    place.blur_radius = motion.blur_radius;
    (place, motion)
}

pub fn mask_window(mask: CanvasMask) -> MaskWindow {
    match mask {
        CanvasMask::None => return MaskWindow::All,
        CanvasMask::Iris { .. } => return MaskWindow::PerPixel,
        CanvasMask::Wipe {
            angle_deg,
            edge,
            keep_below,
        } => {
            let edge = edge.clamp(0.0, 1.0);
            if keep_below && edge <= 1.0e-4 {
                return MaskWindow::Empty;
            }
            if !keep_below && edge >= 1.0 - 1.0e-4 {
                return MaskWindow::Empty;
            }
            if keep_below && edge >= 1.0 - 1.0e-4 {
                return MaskWindow::All;
            }
            if !keep_below && edge <= 1.0e-4 {
                return MaskWindow::All;
            }
            let angle = angle_deg.rem_euclid(360.0);
            let rect = if angle < 0.51 || angle > 359.49 {
                if keep_below {
                    (0.0, 0.0, edge, 1.0)
                } else {
                    (edge, 0.0, 1.0, 1.0)
                }
            } else if (angle - 90.0).abs() < 0.51 {
                if keep_below {
                    (0.0, 0.0, 1.0, edge)
                } else {
                    (0.0, edge, 1.0, 1.0)
                }
            } else if (angle - 180.0).abs() < 0.51 {
                if keep_below {
                    (1.0 - edge, 0.0, 1.0, 1.0)
                } else {
                    (0.0, 0.0, 1.0 - edge, 1.0)
                }
            } else if (angle - 270.0).abs() < 0.51 {
                if keep_below {
                    (0.0, 1.0 - edge, 1.0, 1.0)
                } else {
                    (0.0, 0.0, 1.0, 1.0 - edge)
                }
            } else {
                return MaskWindow::PerPixel;
            };
            if rect.2 - rect.0 < 1.0e-4 || rect.3 - rect.1 < 1.0e-4 {
                MaskWindow::Empty
            } else {
                MaskWindow::Uv {
                    u0: rect.0,
                    v0: rect.1,
                    u1: rect.2,
                    v1: rect.3,
                }
            }
        }
    }
}

pub fn mask_allows(mask: CanvasMask, u: f32, v: f32) -> bool {
    match mask {
        CanvasMask::None => true,
        CanvasMask::Iris { edge, keep_below } => {
            let dx = u - 0.5;
            let dy = v - 0.5;
            let dist = (dx * dx + dy * dy).sqrt() * 2.0_f32.sqrt();
            if keep_below {
                dist <= edge
            } else {
                dist > edge
            }
        }
        CanvasMask::Wipe {
            angle_deg,
            edge,
            keep_below,
        } => {
            let (sin, cos) = angle_deg.to_radians().sin_cos();
            let t = wipe_t(cos, sin, u, v);
            if keep_below {
                t <= edge
            } else {
                t > edge
            }
        }
    }
}

fn wipe_t(nx: f32, ny: f32, u: f32, v: f32) -> f32 {
    let proj = nx * u + ny * v;
    let min = nx.min(0.0) + ny.min(0.0);
    let max = nx.max(0.0) + ny.max(0.0);
    (proj - min) / (max - min).max(1.0e-6)
}

pub fn place_from_transform(xform: &Transform, rel: i64, mix: f32) -> Place {
    Place {
        scale_x: xform.scale_x.value_at(rel).max(0.01),
        scale_y: xform.scale_y.value_at(rel).max(0.01),
        pos_x: xform.position_x.value_at(rel),
        pos_y: xform.position_y.value_at(rel),
        rotation: xform.rotation_deg.value_at(rel),
        anchor_x: xform.anchor_x.value_at(rel),
        anchor_y: xform.anchor_y.value_at(rel),
        opacity: (xform.opacity.value_at(rel) * mix).clamp(0.0, 1.0),
        shift_x: 0.0,
        shift_y: 0.0,
        mask: CanvasMask::None,
        blur_radius: 0.0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub scale_x: f32,
    pub scale_y: f32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub rotation: f32,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub opacity: f32,
    /// Extra sequence-pixel slide. Push transitions set this.
    pub shift_x: f32,
    pub shift_y: f32,
    pub mask: CanvasMask,
    /// Extra blur from blur dissolve or clip filters.
    pub blur_radius: f32,
}

impl Place {
    pub fn identity() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            pos_x: 0.0,
            pos_y: 0.0,
            rotation: 0.0,
            anchor_x: 0.5,
            anchor_y: 0.5,
            opacity: 1.0,
            shift_x: 0.0,
            shift_y: 0.0,
            mask: CanvasMask::None,
            blur_radius: 0.0,
        }
    }

    pub fn contributes(self) -> bool {
        self.opacity > 0.001 && !matches!(mask_window(self.mask), MaskWindow::Empty)
    }
}

pub struct BlitLayer<'a> {
    pub rgba: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub grade: GradeSample,
    pub place: Place,
}

/// Where a program layer's pixels come from. Titles are rasterized at composite
/// time; media is decoded by the caller.
#[derive(Clone, Debug)]
pub enum LayerSource {
    Media {
        path: String,
        source_frame: i64,
        time_secs: f64,
        frame_secs: f64,
        last_source_frame: i64,
    },
    Title(Title),
    /// Solid colour plate (dip-to-white and similar).
    Solid {
        rgb: [f32; 3],
    },
    /// Grade and filters apply to the composite accumulated so far.
    Adjustment,
    /// Child sequence rasterized at `child_frame` during compose.
    Nested {
        sequence_id: editor_core::SequenceId,
        child_frame: i64,
    },
}

/// Shared context for recursively compositing nested sequences.
#[derive(Clone, Copy, Debug)]
pub struct ComposeEnv<'a> {
    pub sequences: &'a [Sequence],
    pub media: &'a [MediaAsset],
    pub groups: &'a [MulticamGroup],
    pub preview_source: crate::PreviewSource,
    pub depth: u32,
}

/// One layer, bottom to top. Higher timeline tracks are later.
#[derive(Clone, Debug)]
pub struct ProgramLayer {
    pub source: LayerSource,
    pub width: u32,
    pub height: u32,
    pub grade: GradeSample,
    pub filters: FilterSample,
    pub place: Place,
    pub label: String,
    /// True when the decoded file is the proxy rather than the camera original.
    pub using_proxy: bool,
    pub clip_id: u64,
}

impl ProgramLayer {
    pub fn is_title(&self) -> bool {
        matches!(self.source, LayerSource::Title(_))
    }

    pub fn is_adjustment(&self) -> bool {
        matches!(self.source, LayerSource::Adjustment)
    }
}

#[derive(Clone, Debug)]
pub struct ProgramStack {
    pub layers: Vec<ProgramLayer>,
    /// Offline or missing media that a visible clip wanted. Preview drops these
    /// when another layer still draws. Export fails on the first one.
    pub errors: Vec<String>,
}

pub fn program_stack(
    sequence: &Sequence,
    media: &[MediaAsset],
    playhead: i64,
    canvas_w: u32,
    canvas_h: u32,
) -> ProgramStack {
    program_stack_with(
        sequence,
        media,
        playhead,
        canvas_w,
        canvas_h,
        crate::PreviewSource::Full,
        &[],
        &[sequence.clone()],
    )
}

/// Like [`program_stack`], but preview may substitute a proxy file.
/// Export keeps calling [`program_stack`], which is always full resolution.
pub fn program_stack_with(
    sequence: &Sequence,
    media: &[MediaAsset],
    playhead: i64,
    canvas_w: u32,
    canvas_h: u32,
    source: crate::PreviewSource,
    groups: &[MulticamGroup],
    sequences: &[Sequence],
) -> ProgramStack {
    let mut layers = Vec::new();
    let mut errors = Vec::new();
    if canvas_w < 2 || canvas_h < 2 {
        return ProgramStack { layers, errors };
    }
    let seq_w = sequence.width.max(1) as f32;
    let seq_h = sequence.height.max(1) as f32;
    for track in sequence.tracks.iter().filter(|track| {
        track.kind == TrackKind::Video && video_track_visible(track, &sequence.tracks)
    }) {
        if let Some(hit) = transition_hit(track, playhead) {
            let mut transition_layers = Vec::new();
            let mut dip_plate: Option<([f32; 3], f32)> = None;
            for (clip, outgoing) in [(hit.left, true), (hit.right, false)] {
                match layer_from_clip(
                    sequence,
                    track,
                    clip,
                    media,
                    groups,
                    sequences,
                    playhead,
                    canvas_w,
                    canvas_h,
                    source,
                ) {
                    Ok(Some(mut layer)) => {
                        let (place, motion) = apply_transition(
                            layer.place,
                            &hit.kind,
                            hit.progress,
                            outgoing,
                            seq_w,
                            seq_h,
                        );
                        layer.place = place;
                        if let Some(plate) = motion.dip_plate {
                            dip_plate = Some(plate);
                        }
                        // The transition can change scale-1 geometry into a slide,
                        // so the decode size has to follow the final place.
                        let (width, height) = layer_pixel_size(canvas_w, canvas_h, &layer.place);
                        layer.width = width;
                        layer.height = height;
                        transition_layers.push(layer);
                    }
                    Ok(None) => {}
                    Err(message) => errors.push(message),
                }
            }
            if let Some((rgb, opacity)) = dip_plate {
                if opacity > 0.001 && !transition_layers.is_empty() {
                    let plate = ProgramLayer {
                        source: LayerSource::Solid { rgb },
                        width: canvas_w,
                        height: canvas_h,
                        grade: GradeSample::neutral(),
                        filters: FilterSample::neutral(),
                        place: Place {
                            opacity,
                            ..Place::identity()
                        },
                        label: "Dip plate".into(),
                        clip_id: 0,
                        using_proxy: false,
                    };
                    let insert_at = transition_layers.len().min(1);
                    transition_layers.insert(insert_at, plate);
                }
            }
            layers.extend(transition_layers);
        } else if let Some(clip) = track
            .clips
            .iter()
            .find(|clip| clip.enabled && clip.covers(Frame(playhead)))
        {
            match layer_from_clip(
                sequence,
                track,
                clip,
                media,
                groups,
                sequences,
                playhead,
                canvas_w,
                canvas_h,
                source,
            ) {
                Ok(Some(layer)) => layers.push(layer),
                Ok(None) => {}
                Err(message) => errors.push(message),
            }
        }
    }
    ProgramStack { layers, errors }
}

pub fn video_track_visible(track: &Track, tracks: &[Track]) -> bool {
    if track.muted {
        return false;
    }
    let any_solo = tracks
        .iter()
        .any(|item| item.kind == track.kind && item.solo);
    if any_solo {
        track.solo
    } else {
        true
    }
}

struct TransHit<'a> {
    kind: TransitionKind,
    progress: f32,
    left: &'a Clip,
    right: &'a Clip,
}

fn transition_hit(track: &Track, playhead: i64) -> Option<TransHit<'_>> {
    for transition in &track.transitions {
        let Some(left) = track
            .clips
            .iter()
            .find(|clip| clip.id == transition.left_clip)
        else {
            continue;
        };
        let Some(right) = track
            .clips
            .iter()
            .find(|clip| clip.id == transition.right_clip)
        else {
            continue;
        };
        let Some(progress) = transition.progress(left.timeline_out, Frame(playhead)) else {
            continue;
        };
        return Some(TransHit {
            kind: transition.kind.clone(),
            progress,
            left,
            right,
        });
    }
    None
}

fn layer_from_clip(
    sequence: &Sequence,
    track: &Track,
    clip: &Clip,
    media: &[MediaAsset],
    groups: &[MulticamGroup],
    sequences: &[Sequence],
    playhead: i64,
    canvas_w: u32,
    canvas_h: u32,
    source: crate::PreviewSource,
) -> Result<Option<ProgramLayer>, String> {
    if !clip.enabled {
        return Ok(None);
    }
    let rel = clip_relative(Frame(playhead), clip.timeline_in);
    let xform = transform(&clip.effects)
        .cloned()
        .unwrap_or_else(Transform::identity);
    let place = place_from_transform(&xform, rel, 1.0);
    if place.opacity <= 0.001 {
        return Ok(None);
    }
    if clip.is_adjustment() {
        return Ok(Some(ProgramLayer {
            source: LayerSource::Adjustment,
            width: canvas_w,
            height: canvas_h,
            grade: GradeSample::from_effects(&clip.effects, rel),
            filters: FilterSample::from_effects(&clip.effects, rel),
            place,
            label: layer_label(track, clip),
            using_proxy: false,
            clip_id: clip.id.0,
        }));
    }
    if let Some(title) = clip.title.clone() {
        let (width, height) = layer_pixel_size(canvas_w, canvas_h, &place);
        return Ok(Some(ProgramLayer {
            source: LayerSource::Title(title),
            width,
            height,
            grade: GradeSample::from_effects(&clip.effects, rel),
            filters: FilterSample::from_effects(&clip.effects, rel),
            place,
            label: layer_label(track, clip),
            using_proxy: false,
            clip_id: clip.id.0,
        }));
    }
    if let Some(binding) = &clip.nested {
        if !sequences.iter().any(|item| item.id == binding.sequence) {
            return Err(format!("Nested sequence missing for {}", clip.name));
        }
        let child_frame = source_frame_at(clip, Frame(playhead), sequence.timebase).0;
        return Ok(Some(ProgramLayer {
            source: LayerSource::Nested {
                sequence_id: binding.sequence,
                child_frame,
            },
            width: canvas_w,
            height: canvas_h,
            grade: GradeSample::from_effects(&clip.effects, rel),
            filters: FilterSample::from_effects(&clip.effects, rel),
            place,
            label: layer_label(track, clip),
            using_proxy: false,
            clip_id: clip.id.0,
        }));
    }
    let resolved_angle = picture_at(clip, groups, media, Frame(playhead), sequence.timebase);
    let media_id = resolved_angle
        .as_ref()
        .map(|hit| hit.media_id)
        .or(clip.media_id)
        .ok_or_else(|| format!("No media linked to {}", clip.name))?;
    let asset = media
        .iter()
        .find(|item| item.id == media_id)
        .ok_or_else(|| format!("Missing media for {}", clip.name))?;
    if !asset.has_video {
        return Err(format!("{} has no picture", asset.name));
    }
    let (resolved, using_proxy) = crate::preview_file(asset, source);
    if !resolved.is_file() {
        return Err(format!("Offline — {} is not on disk", asset.name));
    }
    let (width, height) = layer_pixel_size(canvas_w, canvas_h, &place);
    let sample_tb = resolved_angle
        .as_ref()
        .map(|hit| hit.timebase)
        .unwrap_or(clip.media_timebase);
    let mut source_frame = resolved_angle
        .as_ref()
        .map(|hit| hit.source_frame)
        .unwrap_or_else(|| source_frame_at(clip, Frame(playhead), sequence.timebase).0);
    source_frame = source_frame.max(0);
    let last_source_frame = asset.duration.0.saturating_sub(1).max(0);
    if source_frame > last_source_frame {
        source_frame = last_source_frame;
    }
    let duration_secs = asset.duration.to_seconds(asset.timebase);
    let raw_time = Frame(source_frame).to_seconds(sample_tb);
    let time_secs = crate::clamp_preview_time(raw_time, duration_secs).unwrap_or(0.0);
    let angle_note = resolved_angle
        .as_ref()
        .map(|hit| format!("  ·  {}", hit.name))
        .unwrap_or_default();
    Ok(Some(ProgramLayer {
        source: LayerSource::Media {
            path: resolved.to_string_lossy().into_owned(),
            source_frame,
            time_secs,
            frame_secs: sample_tb.frame_duration_secs().max(1.0e-4),
            last_source_frame,
        },
        width,
        height,
        grade: GradeSample::from_effects(&clip.effects, rel),
        filters: FilterSample::from_effects(&clip.effects, rel),
        place,
        label: {
            let mut label = layer_label(track, clip);
            label.push_str(&angle_note);
            label
        },
        using_proxy,
        clip_id: clip.id.0,
    }))
}

fn layer_label(track: &Track, clip: &Clip) -> String {
    let mut label = format!("{}  {}", track.name, clip.name);
    if let Some(badge) = clip.speed.badge() {
        label.push_str("  ");
        label.push_str(&badge);
    }
    label
}

fn layer_pixel_size(canvas_w: u32, canvas_h: u32, place: &Place) -> (u32, u32) {
    let (w, h) = if identity_geom(place) {
        (canvas_w, canvas_h)
    } else {
        (
            (canvas_w as f32 * place.scale_x).round().max(2.0) as u32,
            (canvas_h as f32 * place.scale_y).round().max(2.0) as u32,
        )
    };
    even_cap(w, h)
}

fn even_cap(width: u32, height: u32) -> (u32, u32) {
    let max = crate::MAX_PREVIEW_DIMENSION;
    let w = (width.min(max).max(2)) & !1;
    let h = (height.min(max).max(2)) & !1;
    (w, h)
}

fn identity_geom(place: &Place) -> bool {
    (place.scale_x - 1.0).abs() < 0.01
        && (place.scale_y - 1.0).abs() < 0.01
        && place.pos_x.abs() < 0.5
        && place.pos_y.abs() < 0.5
        && place.shift_x.abs() < 0.5
        && place.shift_y.abs() < 0.5
        && place.rotation.abs() < 0.05
        && (place.anchor_x - 0.5).abs() < 0.01
        && (place.anchor_y - 0.5).abs() < 0.01
}

pub fn composite(
    dst_w: u32,
    dst_h: u32,
    seq_w: f32,
    seq_h: f32,
    layers: &[BlitLayer<'_>],
) -> Vec<u8> {
    let len = (dst_w as usize)
        .checked_mul(dst_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(0);
    let mut dst = vec![0u8; len];
    if dst_w == 0 || dst_h == 0 || seq_w <= 1.0 || seq_h <= 1.0 {
        return dst;
    }
    for layer in layers {
        composite_layer_over(&mut dst, dst_w, dst_h, seq_w, seq_h, layer);
    }
    dst
}

fn composite_layer_over(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    seq_w: f32,
    seq_h: f32,
    layer: &BlitLayer<'_>,
) {
    if !layer.place.contributes()
        || layer.rgba.len() < 4
        || layer.width == 0
        || layer.height == 0
    {
        return;
    }
    if straight_full_frame(layer, dst_w, dst_h) {
        match mask_window(layer.place.mask) {
            MaskWindow::Empty => {}
            MaskWindow::All => grade_over(dst, dst_w, dst_h, layer, 0, 0, dst_w, dst_h),
            MaskWindow::Uv { u0, v0, u1, v1 } => {
                let x0 = (u0.clamp(0.0, 1.0) * dst_w as f32).floor() as u32;
                let y0 = (v0.clamp(0.0, 1.0) * dst_h as f32).floor() as u32;
                let x1 = (u1.clamp(0.0, 1.0) * dst_w as f32).ceil() as u32;
                let y1 = (v1.clamp(0.0, 1.0) * dst_h as f32).ceil() as u32;
                grade_over(dst, dst_w, dst_h, layer, x0, y0, x1, y1);
            }
            MaskWindow::PerPixel => grade_over_masked(dst, dst_w, dst_h, layer),
        }
    } else {
        blit(dst, dst_w, dst_h, seq_w, seq_h, layer);
    }
}

fn solid_rgba(width: u32, height: u32, rgb: [f32; 3]) -> Vec<u8> {
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let r = (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    for px in rgba.chunks_exact_mut(4) {
        px[0] = r;
        px[1] = g;
        px[2] = b;
        px[3] = 255;
    }
    rgba
}

fn apply_chroma_key_spill(rgb: [f32; 3], key_color: [f32; 3], amount: f32) -> [f32; 3] {
    let key_channel = if key_color[1] >= key_color[0] && key_color[1] >= key_color[2] {
        1
    } else if key_color[0] >= key_color[1] && key_color[0] >= key_color[2] {
        0
    } else {
        2
    };
    let max_other = if key_channel == 0 {
        rgb[1].max(rgb[2])
    } else if key_channel == 1 {
        rgb[0].max(rgb[2])
    } else {
        rgb[0].max(rgb[1])
    };
    let spill = rgb[key_channel] - max_other;
    if spill <= 0.0 {
        return rgb;
    }
    let mut out = rgb;
    out[key_channel] -= spill * amount;
    out
}

fn apply_chroma_key(
    rgba: &mut [u8],
    key_color: [f32; 3],
    tolerance: f32,
    softness: f32,
    spill: f32,
) {
    let tol = tolerance * 0.75 + 0.01;
    let soft = softness * 0.5 + 0.01;
    for pixel in rgba.chunks_exact_mut(4) {
        let r = pixel[0] as f32 / 255.0;
        let g = pixel[1] as f32 / 255.0;
        let b = pixel[2] as f32 / 255.0;
        let dr = r - key_color[0];
        let dg = g - key_color[1];
        let db = b - key_color[2];
        let dist = (dr * dr + dg * dg + db * db).sqrt();
        let matte = smoothstep(tol - soft, tol + soft, dist);
        let mut rgb = [r, g, b];
        if spill > 1.0e-4 && matte > 0.1 {
            rgb = apply_chroma_key_spill(rgb, key_color, spill);
            pixel[0] = (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8;
            pixel[1] = (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8;
            pixel[2] = (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        let existing_alpha = pixel[3] as f32 / 255.0;
        pixel[3] = (matte * existing_alpha * 255.0).round() as u8;
    }
}

fn apply_filters(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    filters: &FilterSample,
    extra_blur: f32,
) {
    if filters.chroma_key_tolerance > 1.0e-4 {
        apply_chroma_key(
            rgba,
            filters.chroma_key_color,
            filters.chroma_key_tolerance,
            filters.chroma_key_softness,
            filters.chroma_key_spill,
        );
    }
    let blur_radius = filters.blur_radius.max(extra_blur);
    if blur_radius >= 0.5 {
        box_blur(rgba, width, height, blur_radius);
    }
    if filters.sharpen > 1.0e-4 {
        sharpen_rgba(rgba, width, height, filters.sharpen);
    }
    if filters.crop.iter().any(|v| *v > 1.0e-4) {
        apply_crop(rgba, width, height, filters.crop);
    }
    if filters.vignette_amount > 1.0e-4 {
        apply_vignette(
            rgba,
            width,
            height,
            filters.vignette_amount,
            filters.vignette_softness,
        );
    }
}

fn box_blur(rgba: &mut [u8], width: u32, height: u32, radius: f32) {
    let radius = radius.round().clamp(1.0, 48.0) as i32;
    let w = width as i32;
    let h = height as i32;
    if w < 2 || h < 2 {
        return;
    }
    let mut tmp = rgba.to_vec();
    for _ in 0..2 {
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                let mut n = 0.0f32;
                for dx in -radius..=radius {
                    let sx = (x + dx).clamp(0, w - 1);
                    let idx = (y as usize * w as usize + sx as usize) * 4;
                    for c in 0..4 {
                        acc[c] += rgba[idx + c] as f32;
                    }
                    n += 1.0;
                }
                let idx = (y as usize * w as usize + x as usize) * 4;
                for c in 0..4 {
                    tmp[idx + c] = (acc[c] / n).round() as u8;
                }
            }
        }
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                let mut n = 0.0f32;
                for dy in -radius..=radius {
                    let sy = (y + dy).clamp(0, h - 1);
                    let idx = (sy as usize * w as usize + x as usize) * 4;
                    for c in 0..4 {
                        acc[c] += tmp[idx + c] as f32;
                    }
                    n += 1.0;
                }
                let idx = (y as usize * w as usize + x as usize) * 4;
                for c in 0..4 {
                    rgba[idx + c] = (acc[c] / n).round() as u8;
                }
            }
        }
    }
}

fn sharpen_rgba(rgba: &mut [u8], width: u32, height: u32, amount: f32) {
    let w = width as i32;
    let h = height as i32;
    if w < 3 || h < 3 {
        return;
    }
    let src = rgba.to_vec();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = (y as usize * w as usize + x as usize) * 4;
            for c in 0..3 {
                let center = src[idx + c] as f32;
                let blur = (src[idx - 4 + c] as f32
                    + src[idx + 4 + c] as f32
                    + src[idx - w as usize * 4 + c] as f32
                    + src[idx + w as usize * 4 + c] as f32)
                    * 0.25;
                let sharp = center + amount * (center - blur);
                rgba[idx + c] = sharp.clamp(0.0, 255.0).round() as u8;
            }
        }
    }
}

fn apply_crop(rgba: &mut [u8], width: u32, height: u32, crop: [f32; 4]) {
    let w = width as i32;
    let h = height as i32;
    let left = (crop[0] * w as f32).round() as i32;
    let right = (crop[1] * w as f32).round() as i32;
    let top = (crop[2] * h as f32).round() as i32;
    let bottom = (crop[3] * h as f32).round() as i32;
    for y in 0..h {
        for x in 0..w {
            if x < left || x >= w - right || y < top || y >= h - bottom {
                let idx = (y as usize * w as usize + x as usize) * 4;
                rgba[idx..idx + 4].fill(0);
            }
        }
    }
}

fn apply_vignette(rgba: &mut [u8], width: u32, height: u32, amount: f32, softness: f32) {
    let w = width as f32;
    let h = height as f32;
    let inner = 1.0 - amount * (0.35 + 0.35 * softness);
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 + 0.5) / w;
            let v = (y as f32 + 0.5) / h;
            let dx = u - 0.5;
            let dy = v - 0.5;
            let dist = (dx * dx + dy * dy).sqrt() * 2.0_f32.sqrt();
            let falloff = smoothstep(inner, 1.0, dist);
            let idx = (y as usize * width as usize + x as usize) * 4;
            for c in 0..3 {
                let v = rgba[idx + c] as f32;
                rgba[idx + c] = (v * (1.0 - falloff * amount)).round() as u8;
            }
        }
    }
}

/// Every media layer that must be decoded to rasterize `layers`, including
/// inside nested sequences.
pub fn media_layers_in_stack(layers: &[ProgramLayer], env: ComposeEnv<'_>) -> Vec<ProgramLayer> {
    let mut out = Vec::new();
    for layer in layers {
        match &layer.source {
            LayerSource::Media { .. } => out.push(layer.clone()),
            LayerSource::Nested {
                sequence_id,
                child_frame,
            } => {
                if env.depth >= MAX_NEST_DEPTH {
                    continue;
                }
                let Some(child) = env.sequences.iter().find(|item| item.id == *sequence_id)
                else {
                    continue;
                };
                let stack = program_stack_with(
                    child,
                    env.media,
                    *child_frame,
                    layer.width,
                    layer.height,
                    env.preview_source,
                    env.groups,
                    env.sequences,
                );
                let child_env = ComposeEnv {
                    depth: env.depth + 1,
                    ..env
                };
                out.extend(media_layers_in_stack(&stack.layers, child_env));
            }
            _ => {}
        }
    }
    out
}

fn rasterize_nested(
    env: ComposeEnv<'_>,
    sequence_id: SequenceId,
    child_frame: i64,
    width: u32,
    height: u32,
    media_rgba: &mut dyn FnMut(&ProgramLayer) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    if env.depth >= MAX_NEST_DEPTH {
        return Err("nested sequence depth limit reached".into());
    }
    let child = env
        .sequences
        .iter()
        .find(|item| item.id == sequence_id)
        .ok_or_else(|| format!("nested sequence {} missing", sequence_id.0))?;
    let stack = program_stack_with(
        child,
        env.media,
        child_frame,
        width,
        height,
        env.preview_source,
        env.groups,
        env.sequences,
    );
    if let Some(error) = stack.errors.first() {
        return Err(error.clone());
    }
    let child_env = ComposeEnv {
        sequences: env.sequences,
        media: env.media,
        groups: env.groups,
        preview_source: env.preview_source,
        depth: env.depth + 1,
    };
    compose_layers_env(
        width,
        height,
        child.width.max(1) as f32,
        child.height.max(1) as f32,
        &stack.layers,
        Some(child_env),
        media_rgba,
    )
}

/// Rasterize title generators and composite every layer. `media_rgba` is only
/// called for decoded picture; titles never go through it. Adjustment layers
/// grade and filter the composite accumulated so far.
pub fn compose_layers(
    dst_w: u32,
    dst_h: u32,
    seq_w: f32,
    seq_h: f32,
    layers: &[ProgramLayer],
    media_rgba: impl FnMut(&ProgramLayer) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    compose_layers_env(
        dst_w,
        dst_h,
        seq_w,
        seq_h,
        layers,
        None,
        media_rgba,
    )
}

/// Like [`compose_layers`], but resolves [`LayerSource::Nested`] recursively.
pub fn compose_layers_env(
    dst_w: u32,
    dst_h: u32,
    seq_w: f32,
    seq_h: f32,
    layers: &[ProgramLayer],
    env: Option<ComposeEnv<'_>>,
    mut media_rgba: impl FnMut(&ProgramLayer) -> Result<Vec<u8>, String>,
) -> Result<Vec<u8>, String> {
    let len = (dst_w as usize)
        .checked_mul(dst_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(0);
    let mut dst = vec![0u8; len];
    if dst_w == 0 || dst_h == 0 || seq_w <= 1.0 || seq_h <= 1.0 {
        return Ok(dst);
    }
    for layer in layers {
        if !layer.place.contributes() || layer.width == 0 || layer.height == 0 {
            continue;
        }
        if matches!(layer.source, LayerSource::Adjustment) {
            apply_adjustment_layer(&mut dst, dst_w, dst_h, layer);
            continue;
        }
        let mut rgba = match &layer.source {
            LayerSource::Title(title) => render_title(title, layer.width, layer.height),
            LayerSource::Media { .. } => media_rgba(layer)?,
            LayerSource::Solid { rgb } => solid_rgba(layer.width, layer.height, *rgb),
            LayerSource::Nested {
                sequence_id,
                child_frame,
            } => {
                let Some(env) = env else {
                    return Err(format!("{} needs a nested compose context", layer.label));
                };
                rasterize_nested(
                    env,
                    *sequence_id,
                    *child_frame,
                    layer.width,
                    layer.height,
                    &mut media_rgba,
                )?
            }
            LayerSource::Adjustment => unreachable!(),
        };
        if rgba.len() < 4 {
            continue;
        }
        if !layer.filters.is_neutral() || layer.place.blur_radius >= 0.5 {
            apply_filters(
                &mut rgba,
                layer.width,
                layer.height,
                &layer.filters,
                layer.place.blur_radius,
            );
        }
        let blit = BlitLayer {
            rgba: &rgba,
            width: layer.width,
            height: layer.height,
            grade: layer.grade.clone(),
            place: layer.place,
        };
        composite_layer_over(&mut dst, dst_w, dst_h, seq_w, seq_h, &blit);
    }
    Ok(dst)
}

fn apply_grade_to_rgba(rgba: &mut [u8], grade: &GradeSample) {
    if grade.is_neutral() {
        return;
    }
    for px in rgba.chunks_exact_mut(4) {
        if px[3] == 0 {
            continue;
        }
        let rgb = grade.apply([
            px[0] as f32 / 255.0,
            px[1] as f32 / 255.0,
            px[2] as f32 / 255.0,
        ]);
        px[0] = (rgb[0] * 255.0).round().clamp(0.0, 255.0) as u8;
        px[1] = (rgb[1] * 255.0).round().clamp(0.0, 255.0) as u8;
        px[2] = (rgb[2] * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

fn apply_adjustment_layer(dst: &mut [u8], width: u32, height: u32, layer: &ProgramLayer) {
    let strength = layer.place.opacity.clamp(0.0, 1.0);
    let masked = !matches!(layer.place.mask, CanvasMask::None);
    let needs_blend = strength < 0.999 || masked;
    let before = if needs_blend {
        Some(dst.to_vec())
    } else {
        None
    };
    if !layer.filters.is_neutral() || layer.place.blur_radius >= 0.5 {
        apply_filters(dst, width, height, &layer.filters, layer.place.blur_radius);
    }
    if !layer.grade.is_neutral() {
        apply_grade_to_rgba(dst, &layer.grade);
    }
    if let Some(before) = before {
        blend_adjustment(dst, &before, width, height, strength, layer.place.mask);
    }
}

fn blend_adjustment(
    dst: &mut [u8],
    before: &[u8],
    width: u32,
    height: u32,
    strength: f32,
    mask: CanvasMask,
) {
    for y in 0..height {
        for x in 0..width {
            let u = (x as f32 + 0.5) / width as f32;
            let v = (y as f32 + 0.5) / height as f32;
            if !mask_allows(mask, u, v) {
                let index = (y as usize * width as usize + x as usize) * 4;
                dst[index..index + 4].copy_from_slice(&before[index..index + 4]);
                continue;
            }
            let index = (y as usize * width as usize + x as usize) * 4;
            let mix = strength;
            for c in 0..3 {
                let old = before[index + c] as f32;
                let new = dst[index + c] as f32;
                dst[index + c] = (old + (new - old) * mix).round().clamp(0.0, 255.0) as u8;
            }
            dst[index + 3] = before[index + 3];
        }
    }
}

fn straight_full_frame(layer: &BlitLayer<'_>, dst_w: u32, dst_h: u32) -> bool {
    layer.width == dst_w && layer.height == dst_h && identity_geom(&layer.place)
}

fn grade_over(
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
    layer: &BlitLayer<'_>,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
) {
    let x0 = x0.min(dst_w).min(layer.width);
    let x1 = x1.min(dst_w).min(layer.width).max(x0);
    let y0 = y0.min(dst_h).min(layer.height);
    let y1 = y1.min(dst_h).min(layer.height).max(y0);
    let neutral = layer.grade.is_neutral();
    for y in y0..y1 {
        for x in x0..x1 {
            let index = (y as usize * dst_w as usize + x as usize) * 4;
            let src_index = (y as usize * layer.width as usize + x as usize) * 4;
            if src_index + 3 >= layer.rgba.len() || index + 3 >= dst.len() {
                continue;
            }
            let src = &layer.rgba[src_index..src_index + 4];
            let rgb = if neutral {
                [
                    src[0] as f32 / 255.0,
                    src[1] as f32 / 255.0,
                    src[2] as f32 / 255.0,
                ]
            } else {
                layer.grade.apply([
                    src[0] as f32 / 255.0,
                    src[1] as f32 / 255.0,
                    src[2] as f32 / 255.0,
                ])
            };
            let alpha = src[3] as f32 / 255.0 * layer.place.opacity;
            over(&mut dst[index..index + 4], rgb, alpha);
        }
    }
}

fn grade_over_masked(dst: &mut [u8], dst_w: u32, dst_h: u32, layer: &BlitLayer<'_>) {
    for y in 0..dst_h.min(layer.height) {
        for x in 0..dst_w.min(layer.width) {
            let u = (x as f32 + 0.5) / dst_w as f32;
            let v = (y as f32 + 0.5) / dst_h as f32;
            if !mask_allows(layer.place.mask, u, v) {
                continue;
            }
            let index = (y as usize * dst_w as usize + x as usize) * 4;
            let src_index = (y as usize * layer.width as usize + x as usize) * 4;
            if src_index + 3 >= layer.rgba.len() || index + 3 >= dst.len() {
                continue;
            }
            let src = &layer.rgba[src_index..src_index + 4];
            let rgb = layer.grade.apply([
                src[0] as f32 / 255.0,
                src[1] as f32 / 255.0,
                src[2] as f32 / 255.0,
            ]);
            let alpha = src[3] as f32 / 255.0 * layer.place.opacity;
            over(&mut dst[index..index + 4], rgb, alpha);
        }
    }
}

fn blit(dst: &mut [u8], dst_w: u32, dst_h: u32, seq_w: f32, seq_h: f32, layer: &BlitLayer<'_>) {
    let place = layer.place;
    let sx = dst_w as f32 / seq_w;
    let sy = dst_h as f32 / seq_h;
    let disp_w = (dst_w as f32 * place.scale_x).max(1.0);
    let disp_h = (dst_h as f32 * place.scale_y).max(1.0);
    let center_x = dst_w as f32 * 0.5 + (place.pos_x + place.shift_x) * sx;
    let center_y = dst_h as f32 * 0.5 - (place.pos_y + place.shift_y) * sy;
    let local_anchor_x = (place.anchor_x - 0.5) * disp_w;
    let local_anchor_y = (0.5 - place.anchor_y) * disp_h;
    let (anchor_rx, anchor_ry) = if place.rotation.abs() < 0.05 {
        (local_anchor_x, local_anchor_y)
    } else {
        rotate(local_anchor_x, local_anchor_y, place.rotation)
    };
    let origin_x = center_x - anchor_rx;
    let origin_y = center_y - anchor_ry;
    let (bx0, by0, bx1, by1) = match mask_window(place.mask) {
        MaskWindow::Empty => return,
        MaskWindow::All | MaskWindow::PerPixel => (0, 0, dst_w as i32, dst_h as i32),
        MaskWindow::Uv { u0, v0, u1, v1 } => (
            (u0.clamp(0.0, 1.0) * dst_w as f32).floor() as i32,
            (v0.clamp(0.0, 1.0) * dst_h as f32).floor() as i32,
            (u1.clamp(0.0, 1.0) * dst_w as f32).ceil() as i32,
            (v1.clamp(0.0, 1.0) * dst_h as f32).ceil() as i32,
        ),
    };
    let radius = (disp_w.hypot(disp_h) * 0.5 + 2.0) as i32;
    let min_y = (origin_y as i32 - radius)
        .clamp(by0, by1)
        .clamp(0, dst_h as i32);
    let max_y = (origin_y as i32 + radius + 1)
        .clamp(by0, by1)
        .clamp(0, dst_h as i32);
    let min_x = (origin_x as i32 - radius)
        .clamp(bx0, bx1)
        .clamp(0, dst_w as i32);
    let max_x = (origin_x as i32 + radius + 1)
        .clamp(bx0, bx1)
        .clamp(0, dst_w as i32);
    let src_w = layer.width as i32;
    let src_h = layer.height as i32;
    let upright = place.rotation.abs() < 0.05;
    let per_pixel = matches!(mask_window(place.mask), MaskWindow::PerPixel);
    for y in min_y..max_y {
        for x in min_x..max_x {
            if per_pixel {
                let u = (x as f32 + 0.5) / dst_w as f32;
                let v = (y as f32 + 0.5) / dst_h as f32;
                if !mask_allows(place.mask, u, v) {
                    continue;
                }
            }
            let (local_x, local_y) = if upright {
                (x as f32 + 0.5 - origin_x, y as f32 + 0.5 - origin_y)
            } else {
                rotate(
                    x as f32 + 0.5 - origin_x,
                    y as f32 + 0.5 - origin_y,
                    -place.rotation,
                )
            };
            if local_x.abs() > disp_w * 0.5 || local_y.abs() > disp_h * 0.5 {
                continue;
            }
            let u = local_x / disp_w + 0.5;
            let v = local_y / disp_h + 0.5;
            let sample = sample(
                layer.rgba,
                src_w,
                src_h,
                u * src_w as f32 - 0.5,
                v * src_h as f32 - 0.5,
            );
            let rgb = layer.grade.apply([sample[0], sample[1], sample[2]]);
            let alpha = sample[3] * place.opacity;
            let index = (y as usize * dst_w as usize + x as usize) * 4;
            if index + 3 < dst.len() {
                over(&mut dst[index..index + 4], rgb, alpha);
            }
        }
    }
}

fn sample(src: &[u8], width: i32, height: i32, x: f32, y: f32) -> [f32; 4] {
    if width <= 0 || height <= 0 {
        return [0.0; 4];
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let p00 = pixel(src, width, height, x0, y0);
    let p10 = pixel(src, width, height, x0 + 1, y0);
    let p01 = pixel(src, width, height, x0, y0 + 1);
    let p11 = pixel(src, width, height, x0 + 1, y0 + 1);
    let mut out = [0.0; 4];
    for channel in 0..4 {
        let top = p00[channel] + (p10[channel] - p00[channel]) * tx;
        let bottom = p01[channel] + (p11[channel] - p01[channel]) * tx;
        out[channel] = top + (bottom - top) * ty;
    }
    out
}

fn pixel(src: &[u8], width: i32, height: i32, x: i32, y: i32) -> [f32; 4] {
    if x < 0 || y < 0 || x >= width || y >= height {
        return [0.0; 4];
    }
    let index = (y as usize * width as usize + x as usize) * 4;
    if index + 3 >= src.len() {
        return [0.0; 4];
    }
    [
        src[index] as f32 / 255.0,
        src[index + 1] as f32 / 255.0,
        src[index + 2] as f32 / 255.0,
        src[index + 3] as f32 / 255.0,
    ]
}

fn over(dst: &mut [u8], rgb: [f32; 3], src_a: f32) {
    if src_a <= 0.001 {
        return;
    }
    let src_a = src_a.clamp(0.0, 1.0);
    let dst_a = dst[3] as f32 / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= 1.0e-4 {
        return;
    }
    for channel in 0..3 {
        let src = rgb[channel];
        let old = dst[channel] as f32 / 255.0;
        let mixed = (src * src_a + old * dst_a * (1.0 - src_a)) / out_a;
        dst[channel] = (mixed.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    dst[3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

fn rotate(x: f32, y: f32, degrees: f32) -> (f32, f32) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}

/// Caption layout shared by the monitor burn-in and export.
///
/// The bitmap is 8×8. Scale 4 at 1080p is a 32px cap, with a 48px bottom
/// margin — the same safe area the previous drawtext used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptionStyle {
    pub scale: u32,
    pub glyph: u32,
    pub margin: u32,
}

pub fn caption_style(canvas_h: u32) -> CaptionStyle {
    let scale = ((canvas_h as f32) / 270.0).round().clamp(1.0, 8.0) as u32;
    let margin = ((canvas_h as f32) * 48.0 / 1080.0).round().clamp(6.0, 96.0) as u32;
    CaptionStyle {
        scale,
        glyph: 8 * scale,
        margin,
    }
}

pub fn active_captions(sequence: &Sequence, frame: i64) -> Vec<String> {
    let mut lines = Vec::new();
    for track in &sequence.tracks {
        if track.kind != TrackKind::Caption || track.muted {
            continue;
        }
        for cue in &track.cues {
            if frame >= cue.timeline_in.0 && frame < cue.timeline_out.0 {
                let text = cue.text.replace(['\n', '\r'], " ").trim().to_string();
                if !text.is_empty() {
                    lines.push(text);
                }
            }
        }
    }
    lines
}

pub fn burn_captions(rgba: &mut [u8], width: u32, height: u32, lines: &[String]) {
    if lines.is_empty() || width < 8 || height < 8 {
        return;
    }
    let style = caption_style(height);
    let gap = style.scale;
    let n = lines.len() as u32;
    let block = style.glyph * n + gap * n.saturating_sub(1);
    let mut y = height.saturating_sub(style.margin.saturating_add(block));
    for line in lines {
        draw_line(rgba, width, height, y, line, style.scale);
        y = y.saturating_add(style.glyph + gap);
    }
}

fn draw_line(rgba: &mut [u8], width: u32, height: u32, y: u32, text: &str, scale: u32) {
    let advance = 8 * scale;
    let chars: Vec<char> = text.chars().take(180).collect();
    if chars.is_empty() {
        return;
    }
    let text_w = advance * chars.len() as u32;
    let mut x = width.saturating_sub(text_w) / 2;
    for ch in &chars {
        if let Some(glyph) = glyph_rows(*ch) {
            stamp_glyph(
                rgba,
                width,
                height,
                x as i32,
                y as i32,
                &glyph,
                scale,
                [0, 0, 0, 255],
                true,
            );
        }
        x = x.saturating_add(advance);
    }
    x = width.saturating_sub(text_w) / 2;
    for ch in &chars {
        if let Some(glyph) = glyph_rows(*ch) {
            stamp_glyph(
                rgba,
                width,
                height,
                x as i32,
                y as i32,
                &glyph,
                scale,
                [255, 255, 255, 255],
                false,
            );
        }
        x = x.saturating_add(advance);
    }
}

fn glyph_rows(ch: char) -> Option<[u8; 8]> {
    font8x8::BASIC_FONTS
        .get(ch)
        .or_else(|| font8x8::BASIC_FONTS.get(ch.to_ascii_uppercase()))
}

fn stamp_glyph(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
    rows: &[u8; 8],
    scale: u32,
    color: [u8; 4],
    outline: bool,
) {
    let scale = scale.max(1) as i32;
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..8 {
            if bits & (1 << col) == 0 {
                continue;
            }
            let x = origin_x + col * scale;
            let y = origin_y + row as i32 * scale;
            if outline {
                fill_rect(
                    rgba,
                    width,
                    height,
                    x - 1,
                    y - 1,
                    scale + 2,
                    scale + 2,
                    color,
                );
            } else {
                fill_rect(rgba, width, height, x, y, scale, scale, color);
            }
        }
    }
}

/// Burn a title into a transparent RGBA buffer the size of its layer.
///
/// Glyphs are the same 8×8 bitmap captions use, scaled to `font_size`. The
/// plate is a straight-alpha bar behind the block. Empty text stays clear.
pub fn render_title(title: &Title, width: u32, height: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    if width < 4 || height < 4 {
        return rgba;
    }
    let lines: Vec<Vec<char>> = title
        .text
        .split('\n')
        .take(8)
        .map(|line| line.chars().take(80).collect())
        .collect();
    if lines
        .iter()
        .all(|line| line.iter().all(|ch| ch.is_whitespace()))
    {
        return rgba;
    }
    let scale = ((title.font_size.clamp(0.02, 0.2) * height as f32) / 8.0)
        .round()
        .clamp(1.0, 40.0) as u32;
    let glyph = 8 * scale;
    let gap = scale.max(1);
    let max_chars = lines.iter().map(|line| line.len()).max().unwrap_or(1) as u32;
    let text_w = glyph * max_chars.max(1);
    let text_h = glyph * lines.len() as u32 + gap * lines.len().saturating_sub(1) as u32;
    let pad_x = glyph / 2;
    let pad_y = (glyph / 3).max(scale);
    let block_w = text_w + pad_x * 2;
    let block_h = text_h + pad_y * 2;
    let anchor_x = (title.x.clamp(0.0, 1.0) * width as f32).round() as i32;
    let anchor_y = (title.y.clamp(0.0, 1.0) * height as f32).round() as i32;
    let left = match title.align {
        editor_core::TextAlign::Left => anchor_x,
        editor_core::TextAlign::Center => anchor_x - block_w as i32 / 2,
        editor_core::TextAlign::Right => anchor_x - block_w as i32,
    };
    let top = anchor_y - block_h as i32 / 2;
    if title.plate > 0.01 {
        let alpha = (title.plate.clamp(0.0, 1.0) * 255.0).round() as u8;
        fill_rect(
            &mut rgba,
            width,
            height,
            left,
            top,
            block_w as i32,
            block_h as i32,
            [0, 0, 0, alpha],
        );
    }
    let color = [
        (title.color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (title.color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (title.color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (title.color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ];
    let mut y = top + pad_y as i32;
    for line in &lines {
        let line_w = glyph * line.len() as u32;
        let line_left = match title.align {
            editor_core::TextAlign::Left => left + pad_x as i32,
            editor_core::TextAlign::Center => left + (block_w as i32 - line_w as i32) / 2,
            editor_core::TextAlign::Right => left + block_w as i32 - pad_x as i32 - line_w as i32,
        };
        stamp_line(
            &mut rgba,
            width,
            height,
            line_left,
            y,
            line,
            scale,
            [0, 0, 0, color[3]],
            true,
        );
        stamp_line(
            &mut rgba, width, height, line_left, y, line, scale, color, false,
        );
        y += glyph as i32 + gap as i32;
    }
    rgba
}

fn stamp_line(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    mut x: i32,
    y: i32,
    chars: &[char],
    scale: u32,
    color: [u8; 4],
    outline: bool,
) {
    let advance = 8 * scale as i32;
    for ch in chars {
        if let Some(glyph) = glyph_rows(*ch) {
            stamp_glyph(rgba, width, height, x, y, &glyph, scale, color, outline);
        }
        x += advance;
    }
}

fn fill_rect(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: [u8; 4],
) {
    for py in y..(y + h) {
        if py < 0 || py >= height as i32 {
            continue;
        }
        for px in x..(x + w) {
            if px < 0 || px >= width as i32 {
                continue;
            }
            let index = (py as usize * width as usize + px as usize) * 4;
            if index + 3 < rgba.len() {
                rgba[index..index + 4].copy_from_slice(&color);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{MediaId, SequenceId, TrackId};

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            pixels.extend_from_slice(&rgba);
        }
        pixels
    }

    fn layer<'a>(src: &'a [u8], w: u32, h: u32, place: Place, grade: GradeSample) -> BlitLayer<'a> {
        BlitLayer {
            rgba: src,
            width: w,
            height: h,
            grade,
            place,
        }
    }

    #[test]
    fn exposure_lifts_a_mid_grey_and_neutral_holds() {
        let grade = GradeSample {
            exposure: 1.0,
            ..GradeSample::neutral()
        };
        let lifted = grade.apply([0.5, 0.5, 0.5]);
        assert!(lifted[0] > 0.9, "{lifted:?}");
        let neutral = GradeSample::neutral().apply([0.25, 0.5, 0.75]);
        assert!((neutral[1] - 0.5).abs() < 1.0e-4);
    }

    #[test]
    fn lift_wheel_adds_red_to_shadows_more_than_highlights() {
        let grade = GradeSample {
            lift: [0.5, 0.0, 0.0],
            ..GradeSample::neutral()
        };
        let dark = grade.apply([0.05, 0.05, 0.05]);
        let bright = grade.apply([0.9, 0.9, 0.9]);
        assert!(dark[0] - 0.05 > bright[0] - 0.9 + 0.02);
    }

    #[test]
    fn luma_curve_lifts_mid_grey() {
        let mut curve = ToneCurve::identity();
        curve.set_point_y(2, 0.65);
        let grade = GradeSample {
            luma_curve: curve,
            ..GradeSample::neutral()
        };
        let out = grade.apply([0.5, 0.5, 0.5]);
        assert!(out[0] > 0.55, "{out:?}");
    }

    #[test]
    fn shadows_lift_darks_more_than_whites_and_warmth_is_red() {
        let shadows = GradeSample {
            shadows: 1.0,
            ..GradeSample::neutral()
        };
        let dark = shadows.apply([0.05, 0.05, 0.05]);
        let bright = shadows.apply([0.9, 0.9, 0.9]);
        assert!(dark[0] - 0.05 > bright[0] - 0.9 + 0.05);
        let warm = GradeSample {
            temperature: 1.0,
            ..GradeSample::neutral()
        }
        .apply([0.5, 0.5, 0.5]);
        assert!(warm[0] > warm[2]);
    }

    #[test]
    fn full_frame_grade_and_opacity_composite() {
        let src = solid(2, 2, [128, 128, 128, 255]);
        let layers = [layer(
            &src,
            2,
            2,
            Place {
                opacity: 0.5,
                ..Place::identity()
            },
            GradeSample {
                exposure: 1.0,
                ..GradeSample::neutral()
            },
        )];
        let out = composite(2, 2, 2.0, 2.0, &layers);
        assert!(out[0] > 180, "{}", out[0]);
        assert!((out[3] as i32 - 128).abs() < 2, "{}", out[3]);
    }

    #[test]
    fn dissolve_is_a_linear_mix_over_an_opaque_plate() {
        let red = solid(2, 2, [200, 0, 0, 255]);
        let blue = solid(2, 2, [0, 0, 200, 255]);
        let layers = [
            layer(&red, 2, 2, Place::identity(), GradeSample::neutral()),
            layer(
                &blue,
                2,
                2,
                Place {
                    opacity: 0.5,
                    ..Place::identity()
                },
                GradeSample::neutral(),
            ),
        ];
        let out = composite(2, 2, 2.0, 2.0, &layers);
        assert!((out[0] as i32 - 100).abs() <= 1, "{}", out[0]);
        assert!((out[2] as i32 - 100).abs() <= 1, "{}", out[2]);
        assert_eq!(out[3], 255);
    }

    #[test]
    fn wipe_angle_zero_keeps_the_left_half_and_ninety_the_top() {
        let src = solid(4, 4, [0, 200, 0, 255]);
        let left = layer(
            &src,
            4,
            4,
            Place {
                mask: CanvasMask::Wipe {
                    angle_deg: 0.0,
                    edge: 0.5,
                    keep_below: true,
                },
                ..Place::identity()
            },
            GradeSample::neutral(),
        );
        let out = composite(4, 4, 4.0, 4.0, &[left]);
        assert_eq!(out[3], 255, "left pixel");
        assert_eq!(out[(2 * 4) as usize + 3], 0, "right pixel");

        let top = layer(
            &src,
            4,
            4,
            Place {
                mask: CanvasMask::Wipe {
                    angle_deg: 90.0,
                    edge: 0.5,
                    keep_below: true,
                },
                ..Place::identity()
            },
            GradeSample::neutral(),
        );
        let out = composite(4, 4, 4.0, 4.0, &[top]);
        assert_eq!(out[3], 255, "top pixel");
        assert_eq!(out[(2 * 4 * 4) as usize + 3], 0, "bottom pixel");
    }

    #[test]
    fn push_left_slides_a_full_frame_off_the_canvas() {
        let src = solid(4, 2, [10, 20, 30, 255]);
        let parked = layer(
            &src,
            4,
            2,
            Place {
                shift_x: 4.0,
                ..Place::identity()
            },
            GradeSample::neutral(),
        );
        let out = composite(4, 2, 4.0, 2.0, &[parked]);
        assert!(out.iter().all(|byte| *byte == 0));
        let motion = transition_motion(
            &TransitionKind::PushSlide {
                direction: Direction::Left,
            },
            0.0,
            false,
            100.0,
            40.0,
        );
        assert!((motion.shift_x - 100.0).abs() < 1.0e-3);
        let mid = transition_motion(
            &TransitionKind::PushSlide {
                direction: Direction::Left,
            },
            0.5,
            true,
            100.0,
            40.0,
        );
        assert!((mid.shift_x - (-50.0)).abs() < 1.0e-3);
    }

    #[test]
    fn dip_to_black_fades_through_empty_at_midpoint() {
        let outgoing = transition_motion(&TransitionKind::DipToBlack, 0.25, true, 100.0, 40.0);
        let incoming = transition_motion(&TransitionKind::DipToBlack, 0.75, false, 100.0, 40.0);
        assert!((outgoing.opacity_scale - 0.5).abs() < 1.0e-3);
        assert!((incoming.opacity_scale - 0.5).abs() < 1.0e-3);
        let mid_out = transition_motion(&TransitionKind::DipToBlack, 0.5, true, 100.0, 40.0);
        let mid_in = transition_motion(&TransitionKind::DipToBlack, 0.5, false, 100.0, 40.0);
        assert!(mid_out.opacity_scale < 1.0e-3);
        assert!(mid_in.opacity_scale < 1.0e-3);
    }

    #[test]
    fn dip_to_white_inserts_a_white_plate_at_midpoint() {
        let motion = transition_motion(&TransitionKind::DipToWhite, 0.5, true, 100.0, 40.0);
        let (rgb, opacity) = motion.dip_plate.unwrap();
        assert!((opacity - 1.0).abs() < 1.0e-3);
        assert!(rgb.iter().all(|c| *c > 0.99));
    }

    #[test]
    fn slide_left_keeps_outgoing_stationary_and_moves_incoming() {
        let out = transition_motion(
            &TransitionKind::Slide {
                direction: Direction::Left,
            },
            0.5,
            true,
            100.0,
            40.0,
        );
        let incoming = transition_motion(
            &TransitionKind::Slide {
                direction: Direction::Left,
            },
            0.5,
            false,
            100.0,
            40.0,
        );
        assert!(out.shift_x.abs() < 1.0e-3);
        assert!((incoming.shift_x - 50.0).abs() < 1.0e-3);
    }

    #[test]
    fn blur_dissolve_peaks_blur_at_the_midpoint() {
        let mid = transition_motion(&TransitionKind::BlurDissolve, 0.5, true, 100.0, 40.0);
        let ends = transition_motion(&TransitionKind::BlurDissolve, 0.0, true, 100.0, 40.0);
        assert!(mid.blur_radius > ends.blur_radius + 10.0);
    }

    #[test]
    fn iris_mask_reveals_from_the_centre() {
        let mask = CanvasMask::Iris {
            edge: 0.5,
            keep_below: true,
        };
        assert!(mask_allows(mask, 0.5, 0.5));
        assert!(!mask_allows(mask, 0.05, 0.05));
        let src = solid(8, 8, [0, 200, 0, 255]);
        let out = composite(
            8,
            8,
            8.0,
            8.0,
            &[layer(
                &src,
                8,
                8,
                Place {
                    mask,
                    ..Place::identity()
                },
                GradeSample::neutral(),
            )],
        );
        let centre = (4 * 8 + 4) * 4;
        assert_eq!(out[centre + 3], 255, "centre pixel");
        assert_eq!(out[3], 0, "corner pixel");
    }

    #[test]
    fn blur_vignette_and_crop_filters_change_pixels() {
        let mut src = vec![0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let idx = (y * 16 + x) * 4;
                src[idx] = if x < 8 { 255 } else { 0 };
                src[idx + 1] = 128;
                src[idx + 2] = 64;
                src[idx + 3] = 255;
            }
        }
        let mut blurred = src.clone();
        apply_filters(
            &mut blurred,
            16,
            16,
            &FilterSample {
                blur_radius: 4.0,
                ..FilterSample::neutral()
            },
            0.0,
        );
        let edge = (8 * 16 + 7) * 4;
        assert_ne!(blurred[edge], src[edge], "blur softens the vertical edge");

        let solid = solid(16, 16, [200, 100, 50, 255]);
        let mut vignetted = solid.clone();
        apply_filters(
            &mut vignetted,
            16,
            16,
            &FilterSample {
                vignette_amount: 0.8,
                vignette_softness: 0.5,
                ..FilterSample::neutral()
            },
            0.0,
        );
        let corner = (15 * 16 + 15) * 4;
        assert!(vignetted[corner] < solid[corner]);

        let mut cropped = solid.clone();
        apply_filters(
            &mut cropped,
            16,
            16,
            &FilterSample {
                crop: [0.25, 0.25, 0.25, 0.25],
                ..FilterSample::neutral()
            },
            0.0,
        );
        assert_eq!(cropped[3], 0, "cropped corner is transparent");
        let centre = (8 * 16 + 8) * 4;
        assert_eq!(cropped[centre + 3], 255);
    }

    #[test]
    fn chroma_key_keys_green_plate() {
        let green = solid(16, 16, [0, 255, 0, 255]);
        let mut keyed = green.clone();
        apply_filters(
            &mut keyed,
            16,
            16,
            &FilterSample {
                chroma_key_color: [0.0, 1.0, 0.0],
                chroma_key_tolerance: 0.35,
                chroma_key_softness: 0.15,
                chroma_key_spill: 0.5,
                ..FilterSample::neutral()
            },
            0.0,
        );
        let centre = (8 * 16 + 8) * 4;
        assert_eq!(keyed[centre + 3], 0, "pure green centre is keyed out");

        let mut composite_plate = vec![0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let idx = (y * 16 + x) * 4;
                if x < 8 {
                    composite_plate[idx] = 0;
                    composite_plate[idx + 1] = 255;
                    composite_plate[idx + 2] = 0;
                } else {
                    composite_plate[idx] = 200;
                    composite_plate[idx + 1] = 80;
                    composite_plate[idx + 2] = 60;
                }
                composite_plate[idx + 3] = 255;
            }
        }
        let mut keyed_split = composite_plate.clone();
        apply_filters(
            &mut keyed_split,
            16,
            16,
            &FilterSample {
                chroma_key_color: [0.0, 1.0, 0.0],
                chroma_key_tolerance: 0.35,
                chroma_key_softness: 0.1,
                chroma_key_spill: 0.0,
                ..FilterSample::neutral()
            },
            0.0,
        );
        let green_side = (8 * 16 + 4) * 4;
        let subject_side = (8 * 16 + 12) * 4;
        assert_eq!(keyed_split[green_side + 3], 0, "green half is transparent");
        assert!(
            keyed_split[subject_side + 3] > 200,
            "subject half stays opaque"
        );
    }

    #[test]
    fn anchor_moves_the_opaque_centroid() {
        let src = solid(4, 4, [255, 255, 255, 255]);
        let centered = composite(
            16,
            16,
            16.0,
            16.0,
            &[layer(
                &src,
                4,
                4,
                Place {
                    scale_x: 0.5,
                    scale_y: 0.5,
                    ..Place::identity()
                },
                GradeSample::neutral(),
            )],
        );
        let corner = composite(
            16,
            16,
            16.0,
            16.0,
            &[layer(
                &src,
                4,
                4,
                Place {
                    scale_x: 0.5,
                    scale_y: 0.5,
                    anchor_x: 0.0,
                    anchor_y: 0.0,
                    ..Place::identity()
                },
                GradeSample::neutral(),
            )],
        );
        let a = centroid(&centered, 16, 16);
        let b = centroid(&corner, 16, 16);
        assert!(b.0 > a.0 + 1.0, "{a:?} -> {b:?}");
        assert!(b.1 < a.1 - 1.0, "{a:?} -> {b:?}");
    }

    fn centroid(rgba: &[u8], w: u32, h: u32) -> (f32, f32) {
        let mut sx = 0.0;
        let mut sy = 0.0;
        let mut n = 0.0;
        for y in 0..h {
            for x in 0..w {
                let index = (y * w + x) as usize * 4;
                if rgba[index + 3] > 200 {
                    sx += x as f32;
                    sy += y as f32;
                    n += 1.0;
                }
            }
        }
        assert!(n > 0.0);
        (sx / n, sy / n)
    }

    #[test]
    fn caption_burn_writes_white_pixels_in_the_safe_area() {
        let mut rgba = vec![0u8; 96 * 64 * 4];
        burn_captions(&mut rgba, 96, 64, &["Hi".into()]);
        let style = caption_style(64);
        let mut white = 0;
        for y in 0..64 {
            for x in 0..96 {
                let index = (y * 96 + x) * 4;
                if rgba[index] > 240 && rgba[index + 1] > 240 && rgba[index + 2] > 240 {
                    white += 1;
                    assert!(
                        y + 2 >= 64 - style.margin as usize - style.glyph as usize,
                        "glyph y {y} is above the caption band"
                    );
                }
            }
        }
        assert!(white > 8, "burned caption produced {white} white pixels");
    }

    #[test]
    fn demo_stacks_pip_above_a_graded_base_and_dissolves() {
        let project = editor_core::demo_project();
        let sequence = project.active().unwrap();
        let pip = program_stack(sequence, &project.media, 60, 320, 180);
        assert!(pip.errors.is_empty(), "{:?}", pip.errors);
        assert_eq!(pip.layers.len(), 3, "base, picture-in-picture, and title");
        assert!(pip.layers[2].is_title());
        assert!(pip.layers[0].place.scale_x > 0.9);
        assert!(pip.layers[1].place.scale_x < 0.5);
        assert!(pip.layers[1].place.pos_x > 400.0);
        assert!(pip.layers[0].grade.temperature > 0.2);
        assert!(pip.layers[0].grade.exposure > 0.1);
        let lines = active_captions(sequence, 60);
        assert!(lines.iter().any(|line| line.contains("ridge")));

        let dissolve = program_stack(sequence, &project.media, 144, 320, 180);
        assert_eq!(dissolve.layers.len(), 2);
        assert!((dissolve.layers[0].place.opacity - 1.0).abs() < 1.0e-3);
        assert!((dissolve.layers[1].place.opacity - 0.5).abs() < 1.0e-3);
        assert!(matches!(dissolve.layers[0].place.mask, CanvasMask::None));

        let wipe = program_stack(sequence, &project.media, 264, 320, 180);
        assert_eq!(wipe.layers.len(), 2);
        match wipe.layers[1].place.mask {
            CanvasMask::Wipe {
                angle_deg,
                edge,
                keep_below,
            } => {
                assert!((angle_deg - 90.0).abs() < 1.0e-3);
                assert!((edge - 0.5).abs() < 1.0e-3);
                assert!(keep_below);
            }
            other => panic!("incoming wipe mask was {other:?}"),
        }
    }

    #[test]
    fn higher_track_is_later_and_a_muted_track_is_absent() {
        let dir = std::env::temp_dir().join(format!("meridian-stack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4");
        std::fs::write(&path, b"media").unwrap();
        let path = path.to_string_lossy().into_owned();
        let mut sequence = Sequence::new(
            SequenceId(1),
            "Cut",
            64,
            36,
            editor_core::Timebase::fps_24(),
        );
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Video, "V2");
        let mut base = Clip::basic(10, 0, 12);
        base.media_id = Some(MediaId(1));
        base.name = "BASE".into();
        let mut top = Clip::basic(11, 0, 12);
        top.media_id = Some(MediaId(1));
        top.name = "TOP".into();
        let mut xform = Transform::identity();
        xform.opacity.base = 0.4;
        top.effects.push(editor_core::Effect::Transform(xform));
        sequence.tracks[0].clips = vec![base];
        sequence.tracks[1].clips = vec![top];
        let asset = MediaAsset {
            id: MediaId(1),
            bin_id: editor_core::BinId(1),
            name: "clip".into(),
            path: path.clone(),
            duration: Frame(24),
            timebase: editor_core::Timebase::fps_24(),
            width: Some(64),
            height: Some(36),
            video_codec: Some("h264".into()),
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: false,
            offline: false,
            proxy_path: None,
        };
        let stack = program_stack(&sequence, &[asset.clone()], 2, 64, 36);
        assert_eq!(stack.layers.len(), 2);
        assert!(stack.layers[0].label.contains("BASE"));
        assert!(stack.layers[1].label.contains("TOP"));
        assert!((stack.layers[1].place.opacity - 0.4).abs() < 1.0e-3);
        sequence.tracks[0].muted = true;
        let stack = program_stack(&sequence, &[asset], 2, 64, 36);
        assert_eq!(stack.layers.len(), 1);
        assert!(stack.layers[0].label.contains("TOP"));
        assert!(!stack.layers[0].using_proxy);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proxy_preview_is_opt_in_and_full_stack_stays_on_the_original() {
        use editor_core::{Clip, MediaAsset, MediaId, Sequence, Timebase, TrackKind};

        let dir = std::env::temp_dir().join(format!("meridian-proxy-stack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.mp4");
        let proxy = dir.join("proxy.mp4");
        std::fs::write(&original, b"full").unwrap();
        std::fs::write(&proxy, b"proxy").unwrap();
        let mut sequence = Sequence::new(SequenceId(1), "Cut", 64, 36, Timebase::fps_24());
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        let mut clip = Clip::basic(10, 0, 12);
        clip.media_id = Some(MediaId(1));
        sequence.tracks[0].clips = vec![clip];
        let asset = MediaAsset {
            id: MediaId(1),
            bin_id: editor_core::BinId(1),
            name: "clip".into(),
            path: original.to_string_lossy().into_owned(),
            duration: editor_core::Frame(24),
            timebase: Timebase::fps_24(),
            width: Some(64),
            height: Some(36),
            video_codec: Some("h264".into()),
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: false,
            offline: false,
            proxy_path: Some(proxy.to_string_lossy().into_owned()),
        };
        let full = program_stack(&sequence, &[asset.clone()], 2, 64, 36);
        assert_eq!(full.layers.len(), 1);
        assert!(!full.layers[0].using_proxy);
        assert!(media_path(&full.layers[0]).ends_with("original.mp4"));
        let preview = program_stack_with(
            &sequence,
            &[asset],
            2,
            64,
            36,
            crate::PreviewSource::Proxy,
            &[],
            &[sequence.clone()],
        );
        assert!(preview.layers[0].using_proxy);
        assert!(media_path(&preview.layers[0]).ends_with("proxy.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn title_raster_honours_colour_alignment_and_composites_over_picture() {
        let mut title = editor_core::Title::new("HI");
        title.plate = 0.0;
        title.color = [1.0, 0.0, 0.0, 1.0];
        title.align = editor_core::TextAlign::Center;
        title.x = 0.5;
        title.y = 0.5;
        title.font_size = 0.2;
        let centered = render_title(&title, 160, 48);
        let (cx, _) = ink_centroid(&centered, 160, 48);
        assert!((cx - 80.0).abs() < 18.0, "centered ink at {cx}");
        assert!(
            centered
                .chunks(4)
                .any(|px| px[0] > 200 && px[1] < 20 && px[2] < 20),
            "title should stamp red glyphs"
        );

        title.align = editor_core::TextAlign::Left;
        title.x = 0.05;
        let left = render_title(&title, 160, 48);
        let (lx, _) = ink_centroid(&left, 160, 48);
        assert!(
            lx < cx - 20.0,
            "left ink {lx} should sit left of center {cx}"
        );

        let blank = editor_core::Title::new("   ");
        let clear = render_title(&blank, 32, 32);
        assert!(clear.iter().all(|byte| *byte == 0));

        let mut sequence = Sequence::new(
            SequenceId(1),
            "Titles",
            160,
            48,
            editor_core::Timebase::fps_24(),
        );
        sequence.add_track(TrackId(2), TrackKind::Video, "V1");
        sequence.add_track(TrackId(3), TrackKind::Video, "V2");
        let dir = std::env::temp_dir().join(format!("meridian-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plate.mp4");
        std::fs::write(&path, b"media").unwrap();
        let mut picture = Clip::basic(4, 0, 12);
        picture.media_id = Some(MediaId(1));
        picture.name = "PLATE".into();
        let lower = Clip::generator(
            5,
            0,
            12,
            editor_core::Timebase::fps_24(),
            editor_core::Title::lower_third("HI"),
        );
        sequence.tracks[0].clips = vec![picture];
        sequence.tracks[1].clips = vec![lower];
        let asset = MediaAsset {
            id: MediaId(1),
            bin_id: editor_core::BinId(1),
            name: "plate".into(),
            path: path.to_string_lossy().into_owned(),
            duration: Frame(24),
            timebase: editor_core::Timebase::fps_24(),
            width: Some(160),
            height: Some(48),
            video_codec: Some("h264".into()),
            audio_codec: None,
            audio_channels: None,
            sample_rate: None,
            has_video: true,
            has_audio: false,
            offline: false,
            proxy_path: None,
        };
        let stack = program_stack(&sequence, &[asset], 2, 160, 48);
        assert!(stack.errors.is_empty(), "{:?}", stack.errors);
        assert_eq!(stack.layers.len(), 2);
        assert!(!stack.layers[0].is_title());
        assert!(stack.layers[1].is_title());
        let blue = solid(160, 48, [0, 0, 180, 255]);
        let out =
            compose_layers(160, 48, 160.0, 48.0, &stack.layers, |_| Ok(blue.clone())).unwrap();
        let mut ink = 0;
        let mut blue_left = 0;
        for pixel in out.chunks(4) {
            if pixel[0] > 200 && pixel[1] > 180 && pixel[2] > 160 {
                ink += 1;
            }
            if pixel[2] > 140 && pixel[0] < 30 {
                blue_left += 1;
            }
        }
        assert!(ink > 8, "composited title produced {ink} light pixels");
        assert!(
            blue_left > 8,
            "picture should remain outside the title plate"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adjustment_layer_grades_below_not_above() {
        let bottom = ProgramLayer {
            source: LayerSource::Solid {
                rgb: [40.0 / 255.0, 0.0, 0.0],
            },
            width: 4,
            height: 4,
            grade: GradeSample::neutral(),
            filters: FilterSample::neutral(),
            place: Place::identity(),
            label: "red".into(),
            using_proxy: false,
            clip_id: 1,
        };
        let adjustment = ProgramLayer {
            source: LayerSource::Adjustment,
            width: 4,
            height: 4,
            grade: GradeSample {
                exposure: 1.0,
                ..GradeSample::neutral()
            },
            filters: FilterSample::neutral(),
            place: Place::identity(),
            label: "adj".into(),
            using_proxy: false,
            clip_id: 2,
        };
        let top = ProgramLayer {
            source: LayerSource::Solid {
                rgb: [0.0, 0.0, 40.0 / 255.0],
            },
            width: 4,
            height: 4,
            grade: GradeSample::neutral(),
            filters: FilterSample::neutral(),
            place: Place {
                opacity: 0.5,
                ..Place::identity()
            },
            label: "blue".into(),
            using_proxy: false,
            clip_id: 3,
        };
        let red_only = compose_layers(4, 4, 4.0, 4.0, &[bottom.clone()], |_| {
            Err("no media".into())
        })
        .unwrap();
        let graded = compose_layers(4, 4, 4.0, 4.0, &[bottom.clone(), adjustment.clone()], |_| {
            Err("no media".into())
        })
        .unwrap();
        assert!(
            graded[0] > red_only[0] + 30,
            "adjustment should brighten the plate below: {} vs {}",
            graded[0],
            red_only[0]
        );
        let without = compose_layers(4, 4, 4.0, 4.0, &[bottom.clone(), top.clone()], |_| {
            Err("no media".into())
        })
        .unwrap();
        let with = compose_layers(
            4,
            4,
            4.0,
            4.0,
            &[bottom, adjustment, top],
            |_| Err("no media".into()),
        )
        .unwrap();
        assert!(
            with[0] > without[0] + 15,
            "graded red should show through the semi-transparent top: {} vs {}",
            with[0],
            without[0]
        );
        assert!(
            (with[2] as i32 - without[2] as i32).abs() <= 2,
            "blue channel should match when the top layer is unchanged: {} vs {}",
            with[2],
            without[2]
        );
    }

    fn media_path(layer: &ProgramLayer) -> &str {
        match &layer.source {
            LayerSource::Media { path, .. } => path,
            LayerSource::Title(_)
            | LayerSource::Solid { .. }
            | LayerSource::Adjustment
            | LayerSource::Nested { .. } => "",
        }
    }

    fn ink_centroid(rgba: &[u8], w: u32, h: u32) -> (f32, f32) {
        let mut sx = 0.0;
        let mut sy = 0.0;
        let mut n = 0.0;
        for y in 0..h {
            for x in 0..w {
                let index = (y * w + x) as usize * 4;
                if rgba[index] > 200 && rgba[index + 3] > 200 {
                    sx += x as f32;
                    sy += y as f32;
                    n += 1.0;
                }
            }
        }
        assert!(n > 0.0, "no ink");
        (sx / n, sy / n)
    }
}
