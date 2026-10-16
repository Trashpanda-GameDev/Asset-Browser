//! Writing looping GIFs: the demo library's animations and the clips of the window. Each makes its
//! own frames (whole ones with clear pixels, or the part that changed); the file is written here.

use std::fs::File;
use std::io::{self, BufWriter};
use std::path::Path;

/// How hard the encoder works on each frame's colours, from 1 (best, slowest) to 30 (fastest).
pub const SPEED: i32 = 10;

/// A GIF being written, playing over and over.
pub struct LoopingGif {
    encoder: gif::Encoder<BufWriter<File>>,
}

impl LoopingGif {
    pub fn create(path: &Path, width: u16, height: u16) -> io::Result<Self> {
        let file = BufWriter::new(File::create(path)?);
        let mut encoder = gif::Encoder::new(file, width, height, &[]).map_err(io::Error::other)?;
        encoder.set_repeat(gif::Repeat::Infinite).map_err(io::Error::other)?;
        Ok(Self { encoder })
    }

    pub fn write(&mut self, frame: &gif::Frame) -> io::Result<()> {
        self.encoder.write_frame(frame).map_err(io::Error::other)
    }
}
