//! Sidechain ducking shared by playback and export.
//!
//! A track ducks when another track's pre-fader peak crosses a threshold.
//! Above that threshold the destination falls by a fixed amount. Attack and
//! release smooth the gain. Playback multiplies that gain in the mix, after
//! the compressor and before the fader. Export maps the same threshold,
//! amount, attack, and release onto ffmpeg `sidechaincompress`.

use serde::{Deserialize, Serialize};

use crate::mix::db_to_linear;

pub const DUCK_SAMPLE_RATE: f32 = 48_000.0;

pub const DUCK_THRESHOLD_DB_MIN: f32 = -60.0;
pub const DUCK_THRESHOLD_DB_MAX: f32 = 0.0;
pub const AMOUNT_DB_MIN: f32 = 0.0;
pub const AMOUNT_DB_MAX: f32 = 24.0;
pub const DUCK_ATTACK_MS_MIN: f32 = 0.1;
pub const DUCK_ATTACK_MS_MAX: f32 = 200.0;
pub const DUCK_RELEASE_MS_MIN: f32 = 10.0;
pub const DUCK_RELEASE_MS_MAX: f32 = 2_000.0;

pub const DEFAULT_DUCK_THRESHOLD_DB: f32 = -20.0;
pub const DEFAULT_DUCK_AMOUNT_DB: f32 = 12.0;
pub const DEFAULT_DUCK_ATTACK_MS: f32 = 10.0;
pub const DEFAULT_DUCK_RELEASE_MS: f32 = 250.0;

/// How far a full-scale key sits above the threshold when mapping Amount onto
/// ffmpeg's ratio. The mixer itself uses a fixed amount once the key crosses
/// the threshold; export reaches that amount as the key approaches full scale.
const EXPORT_RATIO_MAX: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackDuck {
    /// Armed. Settings are kept when this is off.
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    /// Key track id. `None` is no source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u64>,
    /// Sidechain threshold in dBFS. Ducking starts above this level.
    #[serde(
        default = "default_threshold_db",
        skip_serializing_if = "is_default_threshold"
    )]
    pub threshold_db: f32,
    /// Gain reduction in dB while the source is above the threshold.
    #[serde(
        default = "default_amount_db",
        skip_serializing_if = "is_default_amount"
    )]
    pub amount_db: f32,
    /// Attack time in milliseconds (gain falling).
    #[serde(
        default = "default_attack_ms",
        skip_serializing_if = "is_default_attack"
    )]
    pub attack_ms: f32,
    /// Release time in milliseconds (gain returning).
    #[serde(
        default = "default_release_ms",
        skip_serializing_if = "is_default_release"
    )]
    pub release_ms: f32,
}

impl Default for TrackDuck {
    fn default() -> Self {
        Self {
            enabled: false,
            source: None,
            threshold_db: DEFAULT_DUCK_THRESHOLD_DB,
            amount_db: DEFAULT_DUCK_AMOUNT_DB,
            attack_ms: DEFAULT_DUCK_ATTACK_MS,
            release_ms: DEFAULT_DUCK_RELEASE_MS,
        }
    }
}

impl TrackDuck {
    /// Omitted from project JSON when every field is still the default.
    pub fn is_bypass(&self) -> bool {
        *self == Self::default()
    }

    /// Enabled, with a source and a reduction that is large enough to hear.
    pub fn is_active(&self) -> bool {
        self.enabled && self.source.is_some() && self.amount_db >= 0.05
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_threshold_db() -> f32 {
    DEFAULT_DUCK_THRESHOLD_DB
}

fn default_amount_db() -> f32 {
    DEFAULT_DUCK_AMOUNT_DB
}

fn default_attack_ms() -> f32 {
    DEFAULT_DUCK_ATTACK_MS
}

fn default_release_ms() -> f32 {
    DEFAULT_DUCK_RELEASE_MS
}

fn is_default_threshold(value: &f32) -> bool {
    (*value - DEFAULT_DUCK_THRESHOLD_DB).abs() < 0.05
}

fn is_default_amount(value: &f32) -> bool {
    (*value - DEFAULT_DUCK_AMOUNT_DB).abs() < 0.05
}

fn is_default_attack(value: &f32) -> bool {
    (*value - DEFAULT_DUCK_ATTACK_MS).abs() < 0.05
}

fn is_default_release(value: &f32) -> bool {
    (*value - DEFAULT_DUCK_RELEASE_MS).abs() < 0.5
}

pub fn clamp_duck_threshold(db: f32) -> f32 {
    if !db.is_finite() {
        return DEFAULT_DUCK_THRESHOLD_DB;
    }
    db.clamp(DUCK_THRESHOLD_DB_MIN, DUCK_THRESHOLD_DB_MAX)
}

pub fn clamp_duck_amount(db: f32) -> f32 {
    if !db.is_finite() {
        return DEFAULT_DUCK_AMOUNT_DB;
    }
    db.clamp(AMOUNT_DB_MIN, AMOUNT_DB_MAX)
}

pub fn clamp_duck_attack(ms: f32) -> f32 {
    if !ms.is_finite() {
        return DEFAULT_DUCK_ATTACK_MS;
    }
    ms.clamp(DUCK_ATTACK_MS_MIN, DUCK_ATTACK_MS_MAX)
}

pub fn clamp_duck_release(ms: f32) -> f32 {
    if !ms.is_finite() {
        return DEFAULT_DUCK_RELEASE_MS;
    }
    ms.clamp(DUCK_RELEASE_MS_MIN, DUCK_RELEASE_MS_MAX)
}

pub fn format_duck_threshold(db: f32) -> String {
    let db = clamp_duck_threshold(db);
    if db.abs() < 0.05 {
        "0".to_string()
    } else {
        format!("{db:.0}")
    }
}

pub fn format_duck_amount(db: f32) -> String {
    let db = clamp_duck_amount(db);
    if db < 0.05 {
        "0".to_string()
    } else {
        format!("-{db:.0}")
    }
}

fn amp_to_db(amp: f32) -> f32 {
    if !amp.is_finite() || amp <= 1.0e-8 {
        return DUCK_THRESHOLD_DB_MIN - 24.0;
    }
    20.0 * amp.log10()
}

fn envelope_coeff(time_ms: f32) -> f32 {
    let tau = time_ms.max(0.01) * 0.001 * DUCK_SAMPLE_RATE;
    (-1.0 / tau).exp()
}

/// Target gain change in dB. `0` is unity. Negative is the duck amount.
pub fn target_reduction_db(key_peak: f32, duck: &TrackDuck) -> f32 {
    if !duck.is_active() {
        return 0.0;
    }
    let level_db = amp_to_db(key_peak);
    if level_db > clamp_duck_threshold(duck.threshold_db) {
        -clamp_duck_amount(duck.amount_db)
    } else {
        0.0
    }
}

/// ffmpeg ratio so a full-scale key is reduced by about `amount` dB.
pub fn export_duck_ratio(threshold_db: f32, amount_db: f32) -> f32 {
    let span = (-clamp_duck_threshold(threshold_db)).max(0.5);
    let depth = clamp_duck_amount(amount_db).min(span * (1.0 - 1.0 / EXPORT_RATIO_MAX));
    if depth < 0.05 {
        return 1.0;
    }
    (1.0 / (1.0 - depth / span)).clamp(1.0, EXPORT_RATIO_MAX)
}

/// ffmpeg `sidechaincompress` for one ducked track. `None` when inactive.
///
/// The filter's main input is the ducked track and its second input is the
/// source. Ratio is chosen so a full-scale source settles near `amount_db`.
pub fn ffmpeg_duck_filter(duck: &TrackDuck) -> Option<String> {
    if !duck.is_active() {
        return None;
    }
    let threshold = db_to_linear(clamp_duck_threshold(duck.threshold_db)).clamp(0.000976563, 1.0);
    let ratio = export_duck_ratio(duck.threshold_db, duck.amount_db);
    let attack = clamp_duck_attack(duck.attack_ms);
    let release = clamp_duck_release(duck.release_ms);
    Some(format!(
        "sidechaincompress=threshold={threshold:.6}:ratio={ratio:.2}:attack={attack:.2}:release={release:.2}:makeup=1:knee=1:link=maximum:detection=peak:level_sc=1:mix=1"
    ))
}

#[derive(Clone, Debug)]
struct Env {
    id: u64,
    gain_db: f32,
    attack_ms: f32,
    release_ms: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

/// Envelope memory for every ducked track on the bus. Playback keeps one of
/// these for the life of a mix chunk.
#[derive(Clone, Debug, Default)]
pub struct DuckMix {
    envs: Vec<Env>,
}

impl DuckMix {
    pub fn reset_track(&mut self, id: u64) {
        self.envs.retain(|env| env.id != id);
    }

    pub fn retain_tracks(&mut self, ids: &[u64]) {
        self.envs.retain(|env| ids.contains(&env.id));
    }

    /// Step one sample and return the linear gain for `track_id` (1 is unity).
    pub fn gain_for(&mut self, track_id: u64, duck: &TrackDuck, key_peak: f32) -> f32 {
        let attack_ms = clamp_duck_attack(duck.attack_ms);
        let release_ms = clamp_duck_release(duck.release_ms);
        let target = target_reduction_db(key_peak, duck);
        let env = if let Some(env) = self.envs.iter_mut().find(|env| env.id == track_id) {
            if (env.attack_ms - attack_ms).abs() > 0.01 {
                env.attack_ms = attack_ms;
                env.attack_coeff = envelope_coeff(attack_ms);
            }
            if (env.release_ms - release_ms).abs() > 0.01 {
                env.release_ms = release_ms;
                env.release_coeff = envelope_coeff(release_ms);
            }
            env
        } else {
            self.envs.push(Env {
                id: track_id,
                gain_db: 0.0,
                attack_ms,
                release_ms,
                attack_coeff: envelope_coeff(attack_ms),
                release_coeff: envelope_coeff(release_ms),
            });
            self.envs.last_mut().expect("env pushed")
        };
        let coeff = if target < env.gain_db {
            env.attack_coeff
        } else {
            env.release_coeff
        };
        env.gain_db = target + coeff * (env.gain_db - target);
        db_to_linear(env.gain_db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_inactive_and_omit_from_json() {
        let duck = TrackDuck::default();
        assert!(!duck.is_active());
        assert!(duck.is_bypass());
        assert!(ffmpeg_duck_filter(&duck).is_none());
        let json = serde_json::to_string(&duck).unwrap();
        assert_eq!(json, "{}");
        let loaded: TrackDuck = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, duck);
    }

    #[test]
    fn loud_key_targets_the_full_amount_and_a_quiet_key_does_not() {
        let duck = TrackDuck {
            enabled: true,
            source: Some(3),
            threshold_db: -20.0,
            amount_db: 12.0,
            attack_ms: 10.0,
            release_ms: 250.0,
        };
        assert!((target_reduction_db(0.5, &duck) + 12.0).abs() < 1.0e-4);
        assert!(target_reduction_db(0.01, &duck).abs() < 1.0e-4);
        let off = TrackDuck {
            enabled: false,
            ..duck
        };
        assert!(target_reduction_db(0.5, &off).abs() < 1.0e-4);
    }

    #[test]
    fn ffmpeg_filter_uses_threshold_amount_attack_and_release() {
        let duck = TrackDuck {
            enabled: true,
            source: Some(9),
            threshold_db: -24.0,
            amount_db: 18.0,
            attack_ms: 15.0,
            release_ms: 300.0,
        };
        let filter = ffmpeg_duck_filter(&duck).unwrap();
        assert!(filter.starts_with("sidechaincompress="), "{filter}");
        assert!(filter.contains("threshold=0.063096"), "{filter}");
        assert!(filter.contains("ratio=4.00"), "{filter}");
        assert!(filter.contains("attack=15.00"), "{filter}");
        assert!(filter.contains("release=300.00"), "{filter}");
        assert!((export_duck_ratio(-24.0, 18.0) - 4.0).abs() < 1.0e-3);
    }

    #[test]
    fn armed_settings_round_trip_when_fields_are_omitted() {
        let duck = TrackDuck {
            enabled: true,
            source: Some(4),
            ..TrackDuck::default()
        };
        let json = serde_json::to_string(&duck).unwrap();
        assert!(json.contains("\"enabled\":true"), "{json}");
        assert!(json.contains("\"source\":4"), "{json}");
        assert!(!json.contains("threshold"), "{json}");
        let loaded: TrackDuck = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, duck);
        assert!((loaded.threshold_db + 20.0).abs() < 1.0e-4);
        assert!((loaded.amount_db - 12.0).abs() < 1.0e-4);
    }
}
