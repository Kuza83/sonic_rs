//! Audio output through an SDL queue (no callback thread to feed).

use md_core::audio::OUTPUT_RATE;
use sdl2::audio::{AudioQueue, AudioSpecDesired};

use super::sdl_error;

const CHANNELS: u8 = 2;
/// Device buffer in sample frames (~11.6 ms at 44.1 kHz).
const DEVICE_BUFFER: u16 = 512;
/// Bytes per stereo 16-bit pair.
const BYTES_PER_PAIR: u32 = 4;

pub struct Audio {
    queue: AudioQueue<i16>,
}

impl Audio {
    pub fn new(context: &sdl2::Sdl) -> anyhow::Result<Self> {
        let subsystem = context.audio().map_err(sdl_error)?;
        let desired = AudioSpecDesired {
            freq: Some(OUTPUT_RATE as i32),
            channels: Some(CHANNELS),
            samples: Some(DEVICE_BUFFER),
        };
        // SDL converts to the device format if it differs from the request.
        let queue = subsystem
            .open_queue::<i16, _>(None, &desired)
            .map_err(sdl_error)?;
        queue.resume();
        Ok(Self { queue })
    }

    /// Queues interleaved stereo samples.
    pub fn queue(&self, samples: &[i16]) -> anyhow::Result<()> {
        self.queue.queue_audio(samples).map_err(sdl_error)
    }

    /// Stereo pairs still waiting to be played.
    /// Drops the queued audio (a new game starts).
    pub fn clear(&self) {
        self.queue.clear();
    }

    pub fn queued_pairs(&self) -> u32 {
        self.queue.size() / BYTES_PER_PAIR
    }
}
