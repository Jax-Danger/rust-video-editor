//! Preview picture cache. The compositor lives in `editor-media` so the
//! program monitor and Deliver share one grade, transform, and transition path.

pub use editor_media::{
    active_captions, burn_captions, compose_layers, mask_window, transition_motion, CanvasMask,
    GradeSample, LayerSource, MaskWindow, Place, ProgramLayer,
};

pub struct PictureCache {
    pub signature: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
