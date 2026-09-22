//! "Northline — Opening", the example sequence shipped with Meridian.

use crate::effects::{AnimatedF32, ColorGrade, Effect, Transform};
use crate::model::{
    Bin, BinId, CaptionCue, Clip, ClipId, CueId, LabelColor, Marker, MarkerId, MediaAsset, MediaId,
    Project, Sequence, SequenceId, Title, Track, TrackId, TrackKind, Transition, TransitionAlign,
    TransitionId, TransitionKind,
};
use crate::time::{Frame, Timebase};

pub fn demo_project() -> Project {
    let mut project = Project::new("Northline — Opening");
    project.next_id = 1000;

    project.bins = vec![
        Bin {
            id: BinId(1),
            name: "Master".into(),
            parent: None,
        },
        Bin {
            id: BinId(2),
            name: "Interviews".into(),
            parent: Some(BinId(1)),
        },
        Bin {
            id: BinId(3),
            name: "B-Roll".into(),
            parent: Some(BinId(1)),
        },
        Bin {
            id: BinId(4),
            name: "Audio".into(),
            parent: Some(BinId(1)),
        },
    ];

    let tb = Timebase::fps_24();
    project.media = vec![
        asset(
            10,
            2,
            "interview.mp4",
            "samples/media/interview.mp4",
            480,
            tb,
            true,
            true,
            Some("h264"),
            Some("aac"),
            Some(960),
            Some(540),
            false,
        ),
        asset(
            11,
            3,
            "city_broll.mp4",
            "samples/media/city_broll.mp4",
            288,
            tb,
            true,
            true,
            Some("h264"),
            Some("aac"),
            Some(960),
            Some(540),
            false,
        ),
        asset(
            12,
            3,
            "aerial.mp4",
            "samples/media/aerial.mp4",
            192,
            tb,
            true,
            true,
            Some("h264"),
            Some("aac"),
            Some(960),
            Some(540),
            false,
        ),
        asset(
            13,
            4,
            "score.wav",
            "media/score.wav",
            720,
            tb,
            false,
            true,
            None,
            Some("pcm"),
            None,
            None,
            true,
        ),
        asset(
            14,
            4,
            "room_tone.wav",
            "media/room_tone.wav",
            240,
            tb,
            false,
            true,
            None,
            Some("pcm"),
            None,
            None,
            true,
        ),
    ];

    let mut sequence = Sequence::new(SequenceId(100), "Timeline 1", 1920, 1080, tb);
    sequence.in_point = Some(Frame(0));
    sequence.out_point = Some(Frame(408));
    sequence.markers = vec![
        Marker {
            id: MarkerId(501),
            frame: Frame(0),
            duration: 0,
            name: "Opening".into(),
            color: LabelColor::Teal,
            comment: "First light over the ridge.".into(),
        },
        Marker {
            id: MarkerId(502),
            frame: Frame(144),
            duration: 0,
            name: "Cut to city".into(),
            color: LabelColor::Amber,
            comment: String::new(),
        },
    ];

    let mut v1 = Track::new(TrackId(201), TrackKind::Video, "V1");
    let mut v2 = Track::new(TrackId(202), TrackKind::Video, "V2");
    let mut v3 = Track::new(TrackId(203), TrackKind::Video, "V3");
    let mut a1 = Track::new(TrackId(211), TrackKind::Audio, "A1");
    let mut a2 = Track::new(TrackId(212), TrackKind::Audio, "A2");
    let a3 = Track::new(TrackId(213), TrackKind::Audio, "A3");
    let mut c1 = Track::new(TrackId(221), TrackKind::Caption, "C1");

    let mut intv_a = placed(301, 10, "INTV A", 0, 144, 24, 168, 480, LabelColor::Teal);
    let mut intv_a_audio = placed(311, 10, "INTV A", 0, 144, 24, 168, 480, LabelColor::Green);
    link(&mut intv_a, &mut intv_a_audio);
    intv_a.effects.push(Effect::Color(warm_interview()));

    let mut city = placed(302, 11, "CITY", 144, 264, 12, 132, 288, LabelColor::Amber);
    let mut city_audio = placed(312, 11, "CITY", 144, 264, 12, 132, 288, LabelColor::Green);
    link(&mut city, &mut city_audio);
    city.effects.push(Effect::Color(city_grade()));

    let mut intv_b = placed(303, 10, "INTV B", 264, 408, 200, 344, 480, LabelColor::Teal);
    let mut intv_b_audio = placed(
        313,
        10,
        "INTV B",
        264,
        408,
        200,
        344,
        480,
        LabelColor::Green,
    );
    link(&mut intv_b, &mut intv_b_audio);
    intv_b.effects.push(Effect::Color(warm_interview()));

    let mut aerial = placed(304, 12, "AERIAL", 36, 108, 0, 72, 192, LabelColor::Violet);
    aerial.effects.push(Effect::Transform(aerial_transform()));

    let mut opening = Title::lower_third("NORTHLINE");
    opening.color = [0.96, 0.93, 0.86, 1.0];
    let title = Clip::generator(305, 0, 120, tb, opening);

    let score = placed(314, 13, "SCORE", 0, 408, 0, 408, 720, LabelColor::Blue);

    v1.clips = vec![intv_a, city, intv_b];
    v2.clips = vec![aerial];
    v3.clips = vec![title];
    a1.clips = vec![intv_a_audio, city_audio, intv_b_audio];
    a2.clips = vec![score];

    v1.transitions = vec![
        Transition {
            id: TransitionId(601),
            kind: TransitionKind::CrossDissolve,
            left_clip: ClipId(301),
            right_clip: ClipId(302),
            duration: 12,
            alignment: TransitionAlign::Center,
        },
        Transition {
            id: TransitionId(602),
            kind: TransitionKind::Wipe { angle_deg: 90.0 },
            left_clip: ClipId(302),
            right_clip: ClipId(303),
            duration: 10,
            alignment: TransitionAlign::Center,
        },
    ];

    c1.cues = vec![
        cue(401, 12, 72, "We came in over the ridge at first light."),
        cue(402, 156, 230, "The city was already awake."),
        cue(403, 280, 360, "Hold on the skyline."),
    ];

    sequence.tracks = vec![v1, v2, v3, a1, a2, a3, c1];
    project.sequences = vec![sequence];
    project.active_sequence = Some(SequenceId(100));
    project.normalize();
    project
}

fn asset(
    id: u64,
    bin: u64,
    name: &str,
    path: &str,
    duration: i64,
    timebase: Timebase,
    has_video: bool,
    has_audio: bool,
    video_codec: Option<&str>,
    audio_codec: Option<&str>,
    width: Option<u32>,
    height: Option<u32>,
    offline: bool,
) -> MediaAsset {
    MediaAsset {
        id: MediaId(id),
        bin_id: BinId(bin),
        name: name.into(),
        path: path.into(),
        duration: Frame(duration),
        timebase,
        width,
        height,
        video_codec: video_codec.map(str::to_string),
        audio_codec: audio_codec.map(str::to_string),
        audio_channels: has_audio.then_some(2),
        sample_rate: has_audio.then_some(48_000),
        has_video,
        has_audio,
        offline,
        proxy_path: None,
    }
}

/// A long synthetic cut: hundreds of non-overlapping shots of the sample
/// interview, back to back. Used to exercise timeline culling.
pub fn dense_project() -> Project {
    const COUNT: usize = 400;
    const CLIP_LEN: i64 = 48;
    let mut project = Project::new("Dense — Long Cut");
    let tb = Timebase::fps_24();
    project.bins.push(Bin {
        id: BinId(1),
        name: "Master".into(),
        parent: None,
    });
    project.media.push(asset(
        10,
        1,
        "interview.mp4",
        "samples/media/interview.mp4",
        480,
        tb,
        true,
        true,
        Some("h264"),
        Some("aac"),
        Some(960),
        Some(540),
        false,
    ));
    let mut sequence = Sequence::new(SequenceId(2), "Long Cut", 1920, 1080, tb);
    sequence.add_track(TrackId(3), TrackKind::Video, "V1");
    sequence.add_track(TrackId(4), TrackKind::Audio, "A1");
    let labels = [
        LabelColor::Blue,
        LabelColor::Teal,
        LabelColor::Amber,
        LabelColor::Violet,
        LabelColor::Green,
        LabelColor::Rose,
    ];
    let source_room = 480 - CLIP_LEN;
    for index in 0..COUNT {
        let start = index as i64 * CLIP_LEN;
        let source = (index as i64 * 17) % source_room;
        let mut clip = Clip::basic((100 + index) as u64, start, start + CLIP_LEN);
        clip.media_id = Some(MediaId(10));
        clip.name = format!("Shot {:03}", index + 1);
        clip.source_in = Frame(source);
        clip.source_out = Frame(source + CLIP_LEN);
        clip.source_max = Frame(480);
        clip.label = labels[index % labels.len()];
        sequence.tracks[0].clips.push(clip);
    }
    project.sequences.push(sequence);
    project.active_sequence = Some(SequenceId(2));
    project.next_id = (100 + COUNT) as u64;
    project.normalize();
    project
}

fn placed(
    id: u64,
    media: u64,
    name: &str,
    timeline_in: i64,
    timeline_out: i64,
    source_in: i64,
    source_out: i64,
    source_max: i64,
    label: LabelColor,
) -> Clip {
    Clip {
        id: ClipId(id),
        media_id: Some(MediaId(media)),
        name: name.into(),
        timeline_in: Frame(timeline_in),
        timeline_out: Frame(timeline_out),
        source_in: Frame(source_in),
        source_out: Frame(source_out),
        source_min: Frame(0),
        source_max: Frame(source_max),
        media_timebase: Timebase::fps_24(),
        linked: Vec::new(),
        enabled: true,
        effects: Vec::new(),
        label,
        volume: AnimatedF32::constant(1.0),
        title: None,
        adjustment: false,
        speed: crate::model::ClipSpeed::normal(),
        multicam: None,
        nested: None,
        track_matte: None,
    }
}

fn link(a: &mut Clip, b: &mut Clip) {
    a.linked.push(b.id);
    b.linked.push(a.id);
}

fn cue(id: u64, start: i64, end: i64, text: &str) -> CaptionCue {
    CaptionCue {
        id: CueId(id),
        timeline_in: Frame(start),
        timeline_out: Frame(end),
        text: text.into(),
        speaker: Some("Interview".into()),
    }
}

fn warm_interview() -> ColorGrade {
    let mut grade = ColorGrade::neutral();
    grade.exposure.base = 0.15;
    grade.contrast.base = 1.05;
    grade.temperature.base = 0.28;
    grade.saturation.base = 0.92;
    grade
}

fn city_grade() -> ColorGrade {
    let mut grade = ColorGrade::neutral();
    grade.contrast.base = 1.12;
    grade.highlights.base = -0.22;
    grade.shadows.base = 0.16;
    grade.saturation.base = 1.18;
    grade.temperature.base = -0.06;
    grade
}

fn aerial_transform() -> Transform {
    let mut transform = Transform::identity();
    transform.position_x.base = 520.0;
    transform.position_y.base = -260.0;
    transform.scale_x = AnimatedF32::constant(0.34);
    transform.scale_y = AnimatedF32::constant(0.34);
    transform.scale_x.set_key(0, 0.32);
    transform.scale_x.set_key(70, 0.42);
    transform.scale_y.set_key(0, 0.32);
    transform.scale_y.set_key(70, 0.42);
    transform.opacity.base = 1.0;
    transform
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    #[test]
    fn demo_has_picture_sound_captions_and_a_cut() {
        let project = demo_project();
        assert_eq!(project.name, "Northline — Opening");
        let sequence = project.active().unwrap();
        assert_eq!(sequence.tracks.len(), 7);
        let v1 = &sequence.tracks[0];
        assert_eq!(v1.clips.len(), 3);
        assert_eq!(v1.transitions.len(), 2);
        assert!(v1.clips[0].linked.contains(&ClipId(311)));
        assert!(sequence.tracks.iter().any(|t| t.cues.len() == 3));
        let dissolve = &v1.transitions[0];
        assert_eq!(dissolve.range(Frame(144)), (Frame(138), Frame(150)));
        assert!(v1.clips[0].tail_handle() > 6);
        assert!(v1.clips[1].head_handle() >= 6);
        let title = sequence.tracks[2]
            .clips
            .iter()
            .find(|clip| clip.is_title())
            .expect("opening title");
        assert_eq!(title.name, "NORTHLINE");
        assert!(title.covers(Frame(24)));
        assert!(title.title.as_ref().unwrap().text.contains("NORTHLINE"));
        let json = project.to_json_pretty().unwrap();
        let loaded = Project::from_json(&json).unwrap();
        assert_eq!(loaded, project);
    }

    #[test]
    fn sample_file_matches_the_demo_project() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/northline-opening.json"
        );
        let text = std::fs::read_to_string(path).expect("samples/northline-opening.json");
        let loaded = Project::from_json(&text).unwrap();
        assert_eq!(loaded, demo_project());
    }

    #[test]
    fn dense_project_culls_to_a_handful_of_shots() {
        use crate::scale::visible_clip_span;

        let project = dense_project();
        let sequence = project.active().unwrap();
        let clips = &sequence.tracks[0].clips;
        assert_eq!(clips.len(), 400);
        assert!(sequence.end_frame().0 >= 400 * 48);
        let span = visible_clip_span(clips, 10_000, 10_200);
        assert!(!span.is_empty());
        assert!(span.len() <= 6, "visible shots {}", span.len());
        assert!(clips
            .windows(2)
            .all(|pair| pair[0].timeline_out.0 <= pair[1].timeline_in.0));
    }

    #[test]
    fn write_sample_if_requested() {
        if std::env::var("MERIDIAN_WRITE_SAMPLE").ok().as_deref() != Some("1") {
            return;
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../samples/northline-opening.json"
        );
        let json = demo_project().to_json_pretty().unwrap();
        std::fs::write(path, json + "\n").unwrap();
    }
}
