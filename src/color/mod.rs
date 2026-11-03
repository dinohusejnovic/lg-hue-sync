pub(crate) mod black_frame;
pub mod energy;
pub mod gamut;
pub(crate) mod sampling;
pub mod strobe;
pub mod zones;

pub use gamut::HueGamut;
pub use zones::{ActiveRect, ColorProcessor, RgbColor, ZoneSampler};
