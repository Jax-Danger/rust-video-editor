//! Colour, transform, and keyframe model.
//!
//! Parameters are constants until a keyframe is added. Once a parameter has
//! keys, evaluation is clip-relative and ignores the constant except as the
//! value before the first key.

use serde::{Deserialize, Serialize};

use crate::time::Frame;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Hold,
    Linear,
}

impl Default for Interpolation {
    fn default() -> Self {
        Self::Linear
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyframeF32 {
    /// Frame offset from the clip's timeline in-point.
    pub frame: i64,
    pub value: f32,
    #[serde(default)]
    pub interpolation: Interpolation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimatedF32 {
    pub base: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<KeyframeF32>,
}

impl AnimatedF32 {
    pub fn constant(base: f32) -> Self {
        Self {
            base,
            keys: Vec::new(),
        }
    }

    pub fn value_at(&self, clip_relative_frame: i64) -> f32 {
        if self.keys.is_empty() {
            return self.base;
        }
        let mut keys = self.keys.clone();
        keys.sort_by_key(|k| k.frame);
        if clip_relative_frame <= keys[0].frame {
            return keys[0].value;
        }
        if clip_relative_frame >= keys[keys.len() - 1].frame {
            return keys[keys.len() - 1].value;
        }
        for pair in keys.windows(2) {
            let a = &pair[0];
            let b = &pair[1];
            if clip_relative_frame >= a.frame && clip_relative_frame <= b.frame {
                if a.interpolation == Interpolation::Hold || b.frame == a.frame {
                    return a.value;
                }
                let t = (clip_relative_frame - a.frame) as f32 / (b.frame - a.frame) as f32;
                return a.value + (b.value - a.value) * t;
            }
        }
        self.base
    }

    pub fn set_key(&mut self, frame: i64, value: f32) {
        if let Some(existing) = self.keys.iter_mut().find(|k| k.frame == frame) {
            existing.value = value;
        } else {
            self.keys.push(KeyframeF32 {
                frame,
                value,
                interpolation: Interpolation::Linear,
            });
            self.keys.sort_by_key(|k| k.frame);
        }
    }

    pub fn remove_key(&mut self, frame: i64) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| k.frame != frame);
        if self.keys.is_empty() {
            // Keep the last keyed value as the constant so the image doesn't jump.
        }
        self.keys.len() != before
    }

    pub fn has_key(&self, frame: i64) -> bool {
        self.keys.iter().any(|k| k.frame == frame)
    }

    /// Write the value the inspector is showing.
    ///
    /// With no keys, this edits the constant. With keys, it writes a key at
    /// `clip_relative_frame`.
    pub fn write_at(&mut self, clip_relative_frame: i64, value: f32) {
        if self.keys.is_empty() {
            self.base = value;
        } else {
            self.set_key(clip_relative_frame, value);
        }
    }
}

impl Default for AnimatedF32 {
    fn default() -> Self {
        Self::constant(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeParam {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Temperature,
    Tint,
    Saturation,
    LiftRed,
    LiftGreen,
    LiftBlue,
    GammaRed,
    GammaGreen,
    GammaBlue,
    GainRed,
    GainGreen,
    GainBlue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelKind {
    Lift,
    Gamma,
    Gain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelChannel {
    Red,
    Green,
    Blue,
}

/// RGB offset for a lift / gamma / gain wheel. Each channel is keyframeable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RgbWheel {
    pub red: AnimatedF32,
    pub green: AnimatedF32,
    pub blue: AnimatedF32,
}

impl RgbWheel {
    pub fn neutral() -> Self {
        Self {
            red: AnimatedF32::constant(0.0),
            green: AnimatedF32::constant(0.0),
            blue: AnimatedF32::constant(0.0),
        }
    }

    pub fn channel_mut(&mut self, channel: WheelChannel) -> &mut AnimatedF32 {
        match channel {
            WheelChannel::Red => &mut self.red,
            WheelChannel::Green => &mut self.green,
            WheelChannel::Blue => &mut self.blue,
        }
    }

    pub fn channel(&self, channel: WheelChannel) -> &AnimatedF32 {
        match channel {
            WheelChannel::Red => &self.red,
            WheelChannel::Green => &self.green,
            WheelChannel::Blue => &self.blue,
        }
    }

    pub fn values_at(&self, rel: i64) -> [f32; 3] {
        [
            self.red.value_at(rel),
            self.green.value_at(rel),
            self.blue.value_at(rel),
        ]
    }
}

impl Default for RgbWheel {
    fn default() -> Self {
        Self::neutral()
    }
}

/// Monotone luma curve. Endpoints stay at (0, 0) and (1, 1); interior points
/// are editable in the Colour workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneCurve {
    /// Sorted (input, output) pairs in 0…1. The first and last are fixed.
    pub points: Vec<(f32, f32)>,
}

impl ToneCurve {
    pub fn identity() -> Self {
        Self {
            points: vec![
                (0.0, 0.0),
                (0.25, 0.25),
                (0.5, 0.5),
                (0.75, 0.75),
                (1.0, 1.0),
            ],
        }
    }

    /// Evaluate the curve at `x` with linear segments between control points.
    pub fn eval(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if self.points.is_empty() {
            return x;
        }
        let mut pts = self.points.clone();
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if x <= pts[0].0 {
            return pts[0].1;
        }
        if x >= pts[pts.len() - 1].0 {
            return pts[pts.len() - 1].1;
        }
        for pair in pts.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            if x >= x0 && x <= x1 {
                if (x1 - x0).abs() < 1.0e-6 {
                    return y0;
                }
                let t = (x - x0) / (x1 - x0);
                return y0 + (y1 - y0) * t;
            }
        }
        x
    }

    /// Move an interior control point. Index 0 and len-1 are the fixed ends.
    pub fn set_point_y(&mut self, index: usize, y: f32) {
        if index == 0 || index + 1 >= self.points.len() {
            return;
        }
        let y = y.clamp(0.0, 1.0);
        self.points[index].1 = y;
    }
}

impl Default for ToneCurve {
    fn default() -> Self {
        Self::identity()
    }
}

/// Neutral grade: exposure 0 stops, contrast 1, saturation 1, offsets 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorGrade {
    pub exposure: AnimatedF32,
    pub contrast: AnimatedF32,
    pub highlights: AnimatedF32,
    pub shadows: AnimatedF32,
    pub temperature: AnimatedF32,
    pub tint: AnimatedF32,
    pub saturation: AnimatedF32,
    #[serde(default)]
    pub lift: RgbWheel,
    #[serde(default)]
    pub gamma: RgbWheel,
    #[serde(default)]
    pub gain: RgbWheel,
    #[serde(default)]
    pub luma_curve: ToneCurve,
}

impl ColorGrade {
    pub fn neutral() -> Self {
        Self {
            exposure: AnimatedF32::constant(0.0),
            contrast: AnimatedF32::constant(1.0),
            highlights: AnimatedF32::constant(0.0),
            shadows: AnimatedF32::constant(0.0),
            temperature: AnimatedF32::constant(0.0),
            tint: AnimatedF32::constant(0.0),
            saturation: AnimatedF32::constant(1.0),
            lift: RgbWheel::neutral(),
            gamma: RgbWheel::neutral(),
            gain: RgbWheel::neutral(),
            luma_curve: ToneCurve::identity(),
        }
    }

    pub fn wheel_mut(&mut self, kind: WheelKind) -> &mut RgbWheel {
        match kind {
            WheelKind::Lift => &mut self.lift,
            WheelKind::Gamma => &mut self.gamma,
            WheelKind::Gain => &mut self.gain,
        }
    }

    pub fn wheel(&self, kind: WheelKind) -> &RgbWheel {
        match kind {
            WheelKind::Lift => &self.lift,
            WheelKind::Gamma => &self.gamma,
            WheelKind::Gain => &self.gain,
        }
    }

    pub fn param_mut(&mut self, param: GradeParam) -> &mut AnimatedF32 {
        match param {
            GradeParam::Exposure => &mut self.exposure,
            GradeParam::Contrast => &mut self.contrast,
            GradeParam::Highlights => &mut self.highlights,
            GradeParam::Shadows => &mut self.shadows,
            GradeParam::Temperature => &mut self.temperature,
            GradeParam::Tint => &mut self.tint,
            GradeParam::Saturation => &mut self.saturation,
            GradeParam::LiftRed => &mut self.lift.red,
            GradeParam::LiftGreen => &mut self.lift.green,
            GradeParam::LiftBlue => &mut self.lift.blue,
            GradeParam::GammaRed => &mut self.gamma.red,
            GradeParam::GammaGreen => &mut self.gamma.green,
            GradeParam::GammaBlue => &mut self.gamma.blue,
            GradeParam::GainRed => &mut self.gain.red,
            GradeParam::GainGreen => &mut self.gain.green,
            GradeParam::GainBlue => &mut self.gain.blue,
        }
    }

    pub fn param(&self, param: GradeParam) -> &AnimatedF32 {
        match param {
            GradeParam::Exposure => &self.exposure,
            GradeParam::Contrast => &self.contrast,
            GradeParam::Highlights => &self.highlights,
            GradeParam::Shadows => &self.shadows,
            GradeParam::Temperature => &self.temperature,
            GradeParam::Tint => &self.tint,
            GradeParam::Saturation => &self.saturation,
            GradeParam::LiftRed => &self.lift.red,
            GradeParam::LiftGreen => &self.lift.green,
            GradeParam::LiftBlue => &self.lift.blue,
            GradeParam::GammaRed => &self.gamma.red,
            GradeParam::GammaGreen => &self.gamma.green,
            GradeParam::GammaBlue => &self.gamma.blue,
            GradeParam::GainRed => &self.gain.red,
            GradeParam::GainGreen => &self.gain.green,
            GradeParam::GainBlue => &self.gain.blue,
        }
    }
}

impl Default for ColorGrade {
    fn default() -> Self {
        Self::neutral()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformParam {
    PositionX,
    PositionY,
    ScaleX,
    ScaleY,
    Rotation,
    AnchorX,
    AnchorY,
    Opacity,
}

/// Position is in sequence pixels, origin at frame centre.
/// Anchor is normalized (0.5, 0.5 is the clip centre). Scale 1 is 100%.
/// Opacity is 0…1. Rotation is degrees, clockwise in screen space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub position_x: AnimatedF32,
    pub position_y: AnimatedF32,
    pub scale_x: AnimatedF32,
    pub scale_y: AnimatedF32,
    pub rotation_deg: AnimatedF32,
    pub anchor_x: AnimatedF32,
    pub anchor_y: AnimatedF32,
    pub opacity: AnimatedF32,
}

impl Transform {
    pub fn identity() -> Self {
        Self {
            position_x: AnimatedF32::constant(0.0),
            position_y: AnimatedF32::constant(0.0),
            scale_x: AnimatedF32::constant(1.0),
            scale_y: AnimatedF32::constant(1.0),
            rotation_deg: AnimatedF32::constant(0.0),
            anchor_x: AnimatedF32::constant(0.5),
            anchor_y: AnimatedF32::constant(0.5),
            opacity: AnimatedF32::constant(1.0),
        }
    }

    pub fn param_mut(&mut self, param: TransformParam) -> &mut AnimatedF32 {
        match param {
            TransformParam::PositionX => &mut self.position_x,
            TransformParam::PositionY => &mut self.position_y,
            TransformParam::ScaleX => &mut self.scale_x,
            TransformParam::ScaleY => &mut self.scale_y,
            TransformParam::Rotation => &mut self.rotation_deg,
            TransformParam::AnchorX => &mut self.anchor_x,
            TransformParam::AnchorY => &mut self.anchor_y,
            TransformParam::Opacity => &mut self.opacity,
        }
    }

    pub fn param(&self, param: TransformParam) -> &AnimatedF32 {
        match param {
            TransformParam::PositionX => &self.position_x,
            TransformParam::PositionY => &self.position_y,
            TransformParam::ScaleX => &self.scale_x,
            TransformParam::ScaleY => &self.scale_y,
            TransformParam::Rotation => &self.rotation_deg,
            TransformParam::AnchorX => &self.anchor_x,
            TransformParam::AnchorY => &self.anchor_y,
            TransformParam::Opacity => &self.opacity,
        }
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

/// Gaussian-style blur radius in sequence pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlurFilter {
    pub radius: AnimatedF32,
}

impl BlurFilter {
    pub fn off() -> Self {
        Self {
            radius: AnimatedF32::constant(0.0),
        }
    }
}

/// Darken the frame edges. `amount` is strength (0…1); `softness` widens the falloff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VignetteFilter {
    pub amount: AnimatedF32,
    pub softness: AnimatedF32,
}

impl VignetteFilter {
    pub fn off() -> Self {
        Self {
            amount: AnimatedF32::constant(0.0),
            softness: AnimatedF32::constant(0.5),
        }
    }
}

/// Crop insets as fractions of width/height (0…0.45 each). Transparent outside the rect.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CropFilter {
    pub left: AnimatedF32,
    pub right: AnimatedF32,
    pub top: AnimatedF32,
    pub bottom: AnimatedF32,
}

impl CropFilter {
    pub fn off() -> Self {
        Self {
            left: AnimatedF32::constant(0.0),
            right: AnimatedF32::constant(0.0),
            top: AnimatedF32::constant(0.0),
            bottom: AnimatedF32::constant(0.0),
        }
    }
}

/// Unsharp mask strength (0…2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharpenFilter {
    pub amount: AnimatedF32,
}

impl SharpenFilter {
    pub fn off() -> Self {
        Self {
            amount: AnimatedF32::constant(0.0),
        }
    }
}

/// Chroma key (green-screen) filter. Key colour is 0…1 RGB. Tolerance and softness
/// control the matte edge; spill suppression reduces key-colour fringing on foreground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChromaKeyFilter {
    pub key_red: AnimatedF32,
    pub key_green: AnimatedF32,
    pub key_blue: AnimatedF32,
    pub tolerance: AnimatedF32,
    pub softness: AnimatedF32,
    pub spill_suppression: AnimatedF32,
}

impl ChromaKeyFilter {
    pub fn off() -> Self {
        Self {
            key_red: AnimatedF32::constant(0.0),
            key_green: AnimatedF32::constant(1.0),
            key_blue: AnimatedF32::constant(0.0),
            tolerance: AnimatedF32::constant(0.0),
            softness: AnimatedF32::constant(0.15),
            spill_suppression: AnimatedF32::constant(0.5),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShapeMaskKind {
    #[default]
    Rectangle,
    Ellipse,
}

/// Rectangular or elliptical soft mask in layer UV space (0…1). Feather softens
/// the edge; invert swaps inside and outside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeMaskFilter {
    #[serde(default)]
    pub shape: ShapeMaskKind,
    pub center_x: AnimatedF32,
    pub center_y: AnimatedF32,
    pub width: AnimatedF32,
    pub height: AnimatedF32,
    pub feather: AnimatedF32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
}

impl ShapeMaskFilter {
    pub fn off() -> Self {
        Self {
            shape: ShapeMaskKind::Rectangle,
            center_x: AnimatedF32::constant(0.5),
            center_y: AnimatedF32::constant(0.5),
            width: AnimatedF32::constant(1.0),
            height: AnimatedF32::constant(1.0),
            feather: AnimatedF32::constant(0.0),
            invert: false,
        }
    }

    pub fn is_active(&self, rel: i64) -> bool {
        if self.invert {
            return true;
        }
        let w = self.width.value_at(rel);
        let h = self.height.value_at(rel);
        let feather = self.feather.value_at(rel);
        w < 0.999 || h < 0.999 || feather > 1.0e-4
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Baked motion sample for stabilization (cumulative offset from clip start).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StabilizeKeyframe {
    pub frame: i64,
    pub dx: f32,
    pub dy: f32,
    #[serde(default)]
    pub rotation_deg: f32,
}

/// Warp-stabilizer-lite: strength removes shake; smoothing widens the temporal
/// low-pass. Optional `keyframes` hold a pre-analyzed motion path (or load from
/// a `<media>.stabilize.json` sidecar at compose time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StabilizeFilter {
    pub strength: AnimatedF32,
    pub smoothing: AnimatedF32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<StabilizeKeyframe>,
}

impl StabilizeFilter {
    pub fn off() -> Self {
        Self {
            strength: AnimatedF32::constant(0.0),
            smoothing: AnimatedF32::constant(0.5),
            keyframes: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterParam {
    BlurRadius,
    VignetteAmount,
    VignetteSoftness,
    CropLeft,
    CropRight,
    CropTop,
    CropBottom,
    SharpenAmount,
    ChromaKeyRed,
    ChromaKeyGreen,
    ChromaKeyBlue,
    ChromaKeyTolerance,
    ChromaKeySoftness,
    ChromaKeySpillSuppression,
    StabilizeStrength,
    StabilizeSmoothing,
    ShapeMaskCenterX,
    ShapeMaskCenterY,
    ShapeMaskWidth,
    ShapeMaskHeight,
    ShapeMaskFeather,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Color(ColorGrade),
    Transform(Transform),
    Blur(BlurFilter),
    Vignette(VignetteFilter),
    Crop(CropFilter),
    Sharpen(SharpenFilter),
    ChromaKey(ChromaKeyFilter),
    Stabilize(StabilizeFilter),
    ShapeMask(ShapeMaskFilter),
}

pub fn color_grade_mut(effects: &mut Vec<Effect>) -> &mut ColorGrade {
    if !effects.iter().any(|e| matches!(e, Effect::Color(_))) {
        effects.push(Effect::Color(ColorGrade::neutral()));
    }
    match effects.iter_mut().find(|e| matches!(e, Effect::Color(_))) {
        Some(Effect::Color(grade)) => grade,
        _ => unreachable!("color grade just inserted"),
    }
}

pub fn transform_mut(effects: &mut Vec<Effect>) -> &mut Transform {
    if !effects.iter().any(|e| matches!(e, Effect::Transform(_))) {
        effects.push(Effect::Transform(Transform::identity()));
    }
    match effects
        .iter_mut()
        .find(|e| matches!(e, Effect::Transform(_)))
    {
        Some(Effect::Transform(transform)) => transform,
        _ => unreachable!("transform just inserted"),
    }
}

pub fn color_grade(effects: &[Effect]) -> Option<&ColorGrade> {
    effects.iter().find_map(|e| match e {
        Effect::Color(grade) => Some(grade),
        _ => None,
    })
}

pub fn transform(effects: &[Effect]) -> Option<&Transform> {
    effects.iter().find_map(|e| match e {
        Effect::Transform(transform) => Some(transform),
        _ => None,
    })
}

pub fn blur_mut(effects: &mut Vec<Effect>) -> &mut BlurFilter {
    if !effects.iter().any(|e| matches!(e, Effect::Blur(_))) {
        effects.push(Effect::Blur(BlurFilter::off()));
    }
    match effects.iter_mut().find(|e| matches!(e, Effect::Blur(_))) {
        Some(Effect::Blur(filter)) => filter,
        _ => unreachable!("blur filter just inserted"),
    }
}

pub fn vignette_mut(effects: &mut Vec<Effect>) -> &mut VignetteFilter {
    if !effects.iter().any(|e| matches!(e, Effect::Vignette(_))) {
        effects.push(Effect::Vignette(VignetteFilter::off()));
    }
    match effects.iter_mut().find(|e| matches!(e, Effect::Vignette(_))) {
        Some(Effect::Vignette(filter)) => filter,
        _ => unreachable!("vignette filter just inserted"),
    }
}

pub fn crop_mut(effects: &mut Vec<Effect>) -> &mut CropFilter {
    if !effects.iter().any(|e| matches!(e, Effect::Crop(_))) {
        effects.push(Effect::Crop(CropFilter::off()));
    }
    match effects.iter_mut().find(|e| matches!(e, Effect::Crop(_))) {
        Some(Effect::Crop(filter)) => filter,
        _ => unreachable!("crop filter just inserted"),
    }
}

pub fn sharpen_mut(effects: &mut Vec<Effect>) -> &mut SharpenFilter {
    if !effects.iter().any(|e| matches!(e, Effect::Sharpen(_))) {
        effects.push(Effect::Sharpen(SharpenFilter::off()));
    }
    match effects.iter_mut().find(|e| matches!(e, Effect::Sharpen(_))) {
        Some(Effect::Sharpen(filter)) => filter,
        _ => unreachable!("sharpen filter just inserted"),
    }
}

pub fn chroma_key_mut(effects: &mut Vec<Effect>) -> &mut ChromaKeyFilter {
    if !effects.iter().any(|e| matches!(e, Effect::ChromaKey(_))) {
        effects.push(Effect::ChromaKey(ChromaKeyFilter::off()));
    }
    match effects
        .iter_mut()
        .find(|e| matches!(e, Effect::ChromaKey(_)))
    {
        Some(Effect::ChromaKey(filter)) => filter,
        _ => unreachable!("chroma key filter just inserted"),
    }
}

pub fn stabilize_mut(effects: &mut Vec<Effect>) -> &mut StabilizeFilter {
    if !effects.iter().any(|e| matches!(e, Effect::Stabilize(_))) {
        effects.push(Effect::Stabilize(StabilizeFilter::off()));
    }
    match effects
        .iter_mut()
        .find(|e| matches!(e, Effect::Stabilize(_)))
    {
        Some(Effect::Stabilize(filter)) => filter,
        _ => unreachable!("stabilize filter just inserted"),
    }
}

pub fn shape_mask_mut(effects: &mut Vec<Effect>) -> &mut ShapeMaskFilter {
    if !effects.iter().any(|e| matches!(e, Effect::ShapeMask(_))) {
        effects.push(Effect::ShapeMask(ShapeMaskFilter::off()));
    }
    match effects
        .iter_mut()
        .find(|e| matches!(e, Effect::ShapeMask(_)))
    {
        Some(Effect::ShapeMask(filter)) => filter,
        _ => unreachable!("shape mask filter just inserted"),
    }
}

pub fn blur(effects: &[Effect]) -> Option<&BlurFilter> {
    effects.iter().find_map(|e| match e {
        Effect::Blur(filter) => Some(filter),
        _ => None,
    })
}

pub fn vignette(effects: &[Effect]) -> Option<&VignetteFilter> {
    effects.iter().find_map(|e| match e {
        Effect::Vignette(filter) => Some(filter),
        _ => None,
    })
}

pub fn crop(effects: &[Effect]) -> Option<&CropFilter> {
    effects.iter().find_map(|e| match e {
        Effect::Crop(filter) => Some(filter),
        _ => None,
    })
}

pub fn sharpen(effects: &[Effect]) -> Option<&SharpenFilter> {
    effects.iter().find_map(|e| match e {
        Effect::Sharpen(filter) => Some(filter),
        _ => None,
    })
}

pub fn chroma_key(effects: &[Effect]) -> Option<&ChromaKeyFilter> {
    effects.iter().find_map(|e| match e {
        Effect::ChromaKey(filter) => Some(filter),
        _ => None,
    })
}

pub fn stabilize(effects: &[Effect]) -> Option<&StabilizeFilter> {
    effects.iter().find_map(|e| match e {
        Effect::Stabilize(filter) => Some(filter),
        _ => None,
    })
}

pub fn shape_mask(effects: &[Effect]) -> Option<&ShapeMaskFilter> {
    effects.iter().find_map(|e| match e {
        Effect::ShapeMask(filter) => Some(filter),
        _ => None,
    })
}

pub fn has_filter(effects: &[Effect], kind: FilterKind) -> bool {
    effects.iter().any(|e| match (kind, e) {
        (FilterKind::Blur, Effect::Blur(f)) => f.radius.base > 1.0e-4 || !f.radius.keys.is_empty(),
        (FilterKind::Vignette, Effect::Vignette(f)) => {
            f.amount.base > 1.0e-4 || !f.amount.keys.is_empty()
        }
        (FilterKind::Crop, Effect::Crop(f)) => {
            f.left.base > 1.0e-4
                || f.right.base > 1.0e-4
                || f.top.base > 1.0e-4
                || f.bottom.base > 1.0e-4
                || !f.left.keys.is_empty()
                || !f.right.keys.is_empty()
                || !f.top.keys.is_empty()
                || !f.bottom.keys.is_empty()
        }
        (FilterKind::Sharpen, Effect::Sharpen(f)) => {
            f.amount.base > 1.0e-4 || !f.amount.keys.is_empty()
        }
        (FilterKind::ChromaKey, Effect::ChromaKey(f)) => {
            f.tolerance.base > 1.0e-4 || !f.tolerance.keys.is_empty()
        }
        (FilterKind::Stabilize, Effect::Stabilize(f)) => {
            f.strength.base > 1.0e-4 || !f.strength.keys.is_empty()
        }
        (FilterKind::ShapeMask, Effect::ShapeMask(f)) => {
            f.invert
                || f.width.base < 0.999
                || f.height.base < 0.999
                || f.feather.base > 1.0e-4
                || !f.width.keys.is_empty()
                || !f.height.keys.is_empty()
                || !f.feather.keys.is_empty()
        }
        _ => false,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    Blur,
    Vignette,
    Crop,
    Sharpen,
    ChromaKey,
    Stabilize,
    ShapeMask,
}

impl FilterKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Blur => "Blur",
            Self::Vignette => "Vignette",
            Self::Crop => "Crop",
            Self::Sharpen => "Sharpen",
            Self::ChromaKey => "Chroma Key",
            Self::Stabilize => "Stabilize",
            Self::ShapeMask => "Shape Mask",
        }
    }
}

/// Clip-relative frame for keyframes. `playhead` and `timeline_in` are sequence frames.
pub fn clip_relative(playhead: Frame, timeline_in: Frame) -> i64 {
    playhead.0 - timeline_in.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_and_hold_interpolation() {
        let mut anim = AnimatedF32::constant(0.0);
        anim.set_key(0, 0.0);
        anim.set_key(10, 10.0);
        assert!((anim.value_at(5) - 5.0).abs() < 1e-4);
        assert!((anim.value_at(-4) - 0.0).abs() < 1e-4);
        assert!((anim.value_at(40) - 10.0).abs() < 1e-4);
        anim.keys[0].interpolation = Interpolation::Hold;
        assert!((anim.value_at(5) - 0.0).abs() < 1e-4);
        assert!((anim.value_at(10) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn write_at_constant_then_keyed() {
        let mut anim = AnimatedF32::constant(1.0);
        anim.write_at(4, 2.5);
        assert!(anim.keys.is_empty());
        assert!((anim.base - 2.5).abs() < 1e-6);
        anim.set_key(0, 2.5);
        anim.write_at(8, 4.0);
        assert!(anim.has_key(8));
        assert!((anim.value_at(8) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn clip_filters_round_trip_in_json() {
        let effects = vec![
            Effect::Blur(BlurFilter {
                radius: AnimatedF32::constant(6.0),
            }),
            Effect::Vignette(VignetteFilter {
                amount: AnimatedF32::constant(0.4),
                softness: AnimatedF32::constant(0.6),
            }),
            Effect::Crop(CropFilter {
                left: AnimatedF32::constant(0.1),
                right: AnimatedF32::constant(0.05),
                top: AnimatedF32::constant(0.0),
                bottom: AnimatedF32::constant(0.15),
            }),
            Effect::Sharpen(SharpenFilter {
                amount: AnimatedF32::constant(0.8),
            }),
            Effect::ChromaKey(ChromaKeyFilter {
                key_red: AnimatedF32::constant(0.0),
                key_green: AnimatedF32::constant(1.0),
                key_blue: AnimatedF32::constant(0.0),
                tolerance: AnimatedF32::constant(0.35),
                softness: AnimatedF32::constant(0.15),
                spill_suppression: AnimatedF32::constant(0.6),
            }),
            Effect::Stabilize(StabilizeFilter {
                strength: AnimatedF32::constant(0.85),
                smoothing: AnimatedF32::constant(0.6),
                keyframes: vec![StabilizeKeyframe {
                    frame: 0,
                    dx: 0.0,
                    dy: 0.0,
                    rotation_deg: 0.0,
                }],
            }),
            Effect::ShapeMask(ShapeMaskFilter {
                shape: ShapeMaskKind::Ellipse,
                center_x: AnimatedF32::constant(0.5),
                center_y: AnimatedF32::constant(0.45),
                width: AnimatedF32::constant(0.6),
                height: AnimatedF32::constant(0.4),
                feather: AnimatedF32::constant(0.08),
                invert: false,
            }),
        ];
        let json = serde_json::to_string(&effects).unwrap();
        let parsed: Vec<Effect> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, effects);
        assert!(has_filter(&effects, FilterKind::Blur));
        assert!(has_filter(&effects, FilterKind::Vignette));
        assert!(has_filter(&effects, FilterKind::ChromaKey));
        assert!(has_filter(&effects, FilterKind::Stabilize));
        assert!(has_filter(&effects, FilterKind::ShapeMask));
    }

    #[test]
    fn shape_mask_defaults_are_full_frame() {
        let mask = ShapeMaskFilter::off();
        assert!(!mask.is_active(0));
        let mut inverted = mask.clone();
        inverted.invert = true;
        assert!(inverted.is_active(0));
    }

    #[test]
    fn track_matte_binding_round_trip_in_json() {
        use crate::model::{TrackMatteBinding, TrackMatteMode};
        let binding = TrackMatteBinding {
            source_track: 1,
            mode: TrackMatteMode::Luma,
            invert: true,
        };
        let json = serde_json::to_string(&binding).unwrap();
        let parsed: TrackMatteBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, binding);
    }

    #[test]
    fn tone_curve_identity_and_lift() {
        let curve = ToneCurve::identity();
        assert!((curve.eval(0.5) - 0.5).abs() < 1.0e-4);
        let mut lifted = ToneCurve::identity();
        lifted.set_point_y(2, 0.65);
        assert!(lifted.eval(0.5) > 0.5);
        assert!((lifted.eval(0.0) - 0.0).abs() < 1.0e-4);
        assert!((lifted.eval(1.0) - 1.0).abs() < 1.0e-4);
    }
}
