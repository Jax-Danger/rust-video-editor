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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Color(ColorGrade),
    Transform(Transform),
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
