//! Audio analysis, independent of any streaming provider. Feed it a decoded preview and get
//! tempo, loudness, dynamics, loud-spike and energy measurements.

mod analyze;
mod decode;
mod key;
mod spectrum;

pub use analyze::{Analysis, analyze};
pub use decode::decode;
pub use spectrum::{BANDS, SpectrumStream};
