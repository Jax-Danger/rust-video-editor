//! Preview picture cache. The compositor lives in `editor-media` so the
//! program monitor and Deliver share one grade, transform, and transition path.

pub use editor_media::{
    active_captions, burn_captions, composite, mask_window, program_stack, transition_motion,
    BlitLayer, CanvasMask, GradeSample, MaskWindow, Place,
};

pub struct PictureCache {
    pub signature: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
