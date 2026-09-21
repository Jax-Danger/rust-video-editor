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
}
