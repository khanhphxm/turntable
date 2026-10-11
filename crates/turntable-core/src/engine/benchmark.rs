//! Runs file-backed audio through the vendored decoder and the engine's real RingSink.
//! No Session, Connect device, transport controls, output device or configuration directory.

use std::{fs::File, io, path::Path, sync::Arc};

use librespot_playback::{
	audio_backend::Sink,
	convert::Converter,
	decoder::{AudioDecoder, SymphoniaDecoder},
	mixer::NoOpVolume,
	player::CrossfadeSource,
};
use symphonia::core::probe::Hint;

use super::transition::{RingSink, Transitions};
use crate::audio::PcmWriter;

/// A local-file version of the engine's incoming decoder and sink.
pub struct Stream {
	decoder: SymphoniaDecoder,
	sink: RingSink,
}

#[derive(Debug, Clone, Copy)]
pub struct Decoded {
	pub frames: u64,
	pub packets: u64,
}

fn decoder(path: &Path) -> io::Result<SymphoniaDecoder> {
	let mut hint = Hint::new();
	if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
		hint.with_extension(extension);
	}
	SymphoniaDecoder::new(File::open(path)?, hint).map_err(io::Error::other)
}

impl Stream {
	/// `crossfade_ms` overlaps a second independently decoded copy at the beginning of the run.
	/// The same packet alignment, attenuation and sample-counted envelope as playback are used.
	pub fn new(path: &Path, writer: PcmWriter, crossfade_ms: u32) -> io::Result<Self> {
		let decoder = decoder(path)?;
		let mut sink = RingSink::new(writer, Arc::new(Transitions::default()), Box::new(NoOpVolume));
		if crossfade_ms > 0 {
			let outgoing = self::decoder(path)?;
			sink.begin_crossfade(CrossfadeSource::from_decoder(Box::new(outgoing)), crossfade_ms);
		}
		Ok(Self { decoder, sink })
	}

	/// Decodes until EOF, blocking on the production ring at the null output's pace.
	pub fn run(mut self, repeats: u32) -> io::Result<Decoded> {
		let mut decoded = Decoded { frames: 0, packets: 0 };
		let mut converter = Converter::new(None);
		for repeat in 0..repeats {
			if repeat > 0 {
				self.decoder.seek(0).map_err(io::Error::other)?;
				self.sink.new_track();
			}
			while let Some((_, packet)) = self.decoder.next_packet().map_err(io::Error::other)? {
				decoded.frames += u64::try_from(packet.samples().map_err(io::Error::other)?.len() / 2).map_err(io::Error::other)?;
				decoded.packets += 1;
				// The engine disables decoder normalization; unity soft volume is a no-op there.
				// Post-mix normalization is done by Render, as it is during regular playback.
				self.sink.write(packet, &mut converter).map_err(io::Error::other)?;
			}
		}
		Ok(decoded)
	}
}
