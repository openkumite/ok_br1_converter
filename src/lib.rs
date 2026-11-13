//! Bloody Roar (USA, PS1) resource conversion library: disc access, the
//! game's container/compression, TIM/TMD/VAB/XA decoding, skeleton and
//! animation, chain physics and glTF/gallery output.
//!
//! Features: `cli` (default) the command line tool's folder output, gallery and ffmpeg conversions. Without
//! `cli` the library has no file system access, no external programs and no `anyhow` (it builds for wasm32);
//! errors are `Error`, the disc comes in through `disc::Sectors`. `runtime` adds `runtime::Converter`, the
//! conversion into runtime data (the raw game files, the executable's tables, models, stages, music and movies; files
//! out through a `runtime::Sink`), step by step, for a program that converts in-app.

pub mod anim;
pub mod audio;
pub mod chainsim;
pub mod chars;
#[cfg(feature = "runtime")]
pub mod clip_bank;
pub mod disc;
pub mod error;
#[cfg(feature = "cli")]
pub mod gallery;
pub mod gltf;
#[cfg(feature = "cli")]
pub mod media;
pub mod model;
pub mod moves;
#[cfg(feature = "runtime")]
pub mod okt;
pub mod pack;
#[cfg(feature = "runtime")]
pub mod runtime;
pub mod stagebg;
#[cfg(feature = "cli")]
pub mod textures;
pub mod tim;
pub mod tmd;
pub mod vfx;
pub mod vram;

pub use error::{Error, Result};
