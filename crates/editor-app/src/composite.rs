//! Preview picture cache. The compositor lives in `editor-media` so the
//! program monitor and Deliver share one grade, transform, and transition path.
//!
//! `PictureCache::rgba` stays on the CPU. The program monitor may upload a
//! copy to the GPU for display; Deliver never reads that texture.

pub use editor_media::{
    active_captions, burn_captions, compose_layers, compose_layers_env, mask_window,
    media_layers_in_stack, transition_motion, CanvasMask, ComposeEnv, FilterSample, GradeSample,
    LayerSource, MaskWindow, Place, ProgramLayer, StabilizeSample,
};

pub struct PictureCache {
    pub signature: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
