//! Export planning. Picture encoding is intentionally not linked in this build;
//! the plan is a concrete manifest a later encoder can consume.

use serde::{Deserialize, Serialize};

use crate::model::{Sequence, TrackKind};
use crate::time::Frame;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportPlan {
    pub sequence_name: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub codec: String,
    pub container: String,
    pub in_frame: i64,
    pub out_frame: i64,
    pub output_path: String,
    pub video_clips: usize,
    pub audio_clips: usize,
    pub caption_cues: usize,
    pub markers: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRange {
    WholeSequence,
    InOut,
}

pub fn plan_export(
    sequence: &Sequence,
    codec: &str,
    container: &str,
    output_path: &str,
    range: ExportRange,
) -> ExportPlan {
    let end = sequence.end_frame();
    let (in_frame, out_frame) = match range {
        ExportRange::InOut => {
            let in_frame = sequence.in_point.unwrap_or(Frame::ZERO).0.max(0);
            let out_frame = sequence
                .out_point
                .map(|f| f.0)
                .unwrap_or(end.0)
                .max(in_frame);
            (in_frame, out_frame)
        }
        ExportRange::WholeSequence => (0, end.0),
    };
    let mut video_clips = 0;
    let mut audio_clips = 0;
    let mut caption_cues = 0;
    for track in &sequence.tracks {
        match track.kind {
            TrackKind::Video => video_clips += track.clips.len(),
            TrackKind::Audio => audio_clips += track.clips.len(),
            TrackKind::Caption => caption_cues += track.cues.len(),
        }
    }
    ExportPlan {
        sequence_name: sequence.name.clone(),
        width: sequence.width,
        height: sequence.height,
        fps_num: sequence.timebase.numerator,
        fps_den: sequence.timebase.denominator,
        codec: codec.to_string(),
        container: container.to_string(),
        in_frame,
        out_frame,
        output_path: output_path.to_string(),
        video_clips,
        audio_clips,
        caption_cues,
        markers: sequence.markers.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SequenceId, TrackId, TrackKind};
    use crate::time::Timebase;

    #[test]
    fn in_out_range_is_honoured() {
        let mut seq = Sequence::new(SequenceId(1), "Timeline", 1920, 1080, Timebase::fps_24());
        seq.add_track(TrackId(2), TrackKind::Video, "V1");
        seq.in_point = Some(Frame(10));
        seq.out_point = Some(Frame(40));
        let plan = plan_export(&seq, "H.264", "mp4", "/tmp/out.json", ExportRange::InOut);
        assert_eq!(plan.in_frame, 10);
        assert_eq!(plan.out_frame, 40);
        assert_eq!(plan.fps_num, 24);
    }
}
