//! Immediate-mode panels for the Meridian shell.

mod audio_slot;
mod captions;
mod deliver;
mod inspector;
mod pool;
mod timeline;
mod viewer;
pub mod widgets;

use editor_core::{Frame, Timebase};

pub use audio_slot::audio_workspace;
pub use captions::captions_panel;
pub use deliver::deliver_panel;
pub use inspector::inspector_panel;
pub use pool::media_pool;
pub use timeline::timeline_panel;
pub use viewer::viewer_panel;

pub(crate) fn media_missing(path: &str) -> bool {
    path.is_empty() || !editor_media::resolve_media_path(path).is_file()
}

pub fn format_tc(frame: i64, timebase: Timebase) -> String {
    if timebase.timecode_fps() > 120 {
        format!("{:.3}s", Frame(frame).to_seconds(timebase))
    } else {
        Frame(frame).format_timecode(timebase)
    }
}
