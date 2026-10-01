//! Handling various protocols pioneered by Kitty,
//! including the [Kitty graphics protocol](graphics) and drops through the
//! [Kitty drag and drop protocol](dnd).

pub mod dnd;
pub mod graphics;

#[cfg(feature = "kitty-graphics")]
pub use graphics::Graphics;
