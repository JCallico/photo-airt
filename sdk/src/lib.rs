//! Photo·AIrt style SDK: everything a style written in Rust needs.
//!
//! * [`imaging`]: the float `Img` buffer and shared raster primitives.
//! * The contract: the [`Manifest`] (`plugin.toml`), [`Description`],
//!   [`Setting`], [`Params`], [`Ctx`] and the [`Style`] pair of description
//!   and render functions.
//! * [`serve`]: the `main` of a standalone plug-in executable, speaking the
//!   external contract (`render <request.json>`).
//!
//! The built-in styles in `plugins/` are crates built on this SDK. The app
//! links them in, and each also builds a standalone executable, so any of
//! them can run as an external plug-in.

mod contract;
pub mod imaging;
mod serve;

pub use contract::*;
pub use serve::{read_png, render_command, serve, write_png};
