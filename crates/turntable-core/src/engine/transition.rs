//! Natural transitions: two decoders, one equal-power mixer, one unchanged DSP chain.
//!
//! Policy: crossfade songs within a context, except adjacent tracks in the
//! original order of an unshuffled album context (including disc boundaries).
//! Those transitions preserve the decoder's gapless trimming and concatenate
//! PCM without closing/clearing the ring, with only an eight-millisecond click ramp at track starts.
//! Playlists, shuffled albums, queued
//! nonadjacent songs and album wraparound may crossfade; episodes never do.
//! This deliberately does not implement Spotify Automix. Unknown album order
//! stays gapless until Session metadata supplies it, rather than risking a fade
//! across a continuous album. User loads, next/previous, seeks and stops cancel
//! the outgoing decoder and cut buffered old PCM at the next render callback.
//!
//! A short song bounds the overlap to its available duration; an active overlap
//! never starts another one. Decoder packets round the start by at most one
//! packet; envelopes are sample-counted. Pause freezes both decoders; changing
//! the preference affects the next transition, not a running envelope.

use std::sync::{Arc, RwLock};

use librespot_core::SpotifyUri;
use librespot_metadata::Album;
use librespot_playback::{
	audio_backend::{Sink, SinkError, SinkResult},
	convert::Converter,
	decoder::AudioPacket,
	mixer::VolumeGetter,
	player::CrossfadeSource,
};

use crate::audio::{CLICK_FADE_FRAMES, Crossfade, PcmWriter, dsp_f32};

#[derive(Default)]
struct Context {
	uri:     String,
	shuffle: bool,
	album:   Vec<SpotifyUri>,
}

/// Context order is published on the engine thread, read only by the decoder.
#[derive(Default)]
pub(super) struct Transitions(RwLock<Context>);

impl Transitions {
	pub(super) fn context(&self, uri: &str, shuffle: bool) {
		let mut context = self.0.write().unwrap_or_else(std::sync::PoisonError::into_inner);
		if context.uri != uri {
			context.uri.clear();
			context.uri.push_str(uri);
			context.album.clear();
		}
		context.shuffle = shuffle;
	}

	pub(super) fn album(&self, album: &Album) {
		let mut context = self.0.write().unwrap_or_else(std::sync::PoisonError::into_inner);
		context.album.clear();
		context.album.extend(album.tracks().cloned());
	}

	fn allows(&self, current: &SpotifyUri, next: &SpotifyUri) -> bool {
		if !matches!((current, next), (SpotifyUri::Track { .. }, SpotifyUri::Track { .. })) {
			return false;
		}
		let context = self.0.read().unwrap_or_else(std::sync::PoisonError::into_inner);
		if context.uri.is_empty() {
			return false;
		}
		if !context.uri.starts_with("spotify:album:") || context.shuffle {
			return true;
		}
		!context.album.is_empty() && !context.album.windows(2).any(|pair| pair.first() == Some(current) && pair.get(1) == Some(next))
	}
}

/// librespot's output; only an overlap decodes from a second source.
pub(super) struct RingSink {
	writer:          PcmWriter,
	transitions:     Arc<Transitions>,
	volume:          Box<dyn VolumeGetter + Send>,
	samples:         Vec<f32>,
	outgoing:        Vec<f32>,
	packet:          Vec<f32>,
	cursor:          usize,
	source:          Option<CrossfadeSource>,
	fade:            Crossfade,
	start_remaining: usize,
}

impl RingSink {
	pub(super) fn new(writer: PcmWriter, transitions: Arc<Transitions>, volume: Box<dyn VolumeGetter + Send>) -> Self {
		Self {
			writer,
			transitions,
			volume,
			samples: Vec::new(),
			outgoing: Vec::new(),
			packet: Vec::new(),
			cursor: 0,
			source: None,
			fade: Crossfade::default(),
			start_remaining: CLICK_FADE_FRAMES,
		}
	}

	/// Aligns independently sized decoder packets without dropping either channel.
	fn read_outgoing(&mut self, samples: usize) -> SinkResult<()> {
		self.outgoing.clear();
		let volume = self.volume.attenuation_factor();
		while self.outgoing.len() < samples {
			if let Some(available) = self.packet.get(self.cursor..).filter(|tail| !tail.is_empty()) {
				let count = available.len().min(samples - self.outgoing.len());
				self.outgoing.extend(available.iter().take(count).map(|s| dsp_f32(f64::from(*s) * volume)));
				self.cursor += count;
				continue;
			}
			let Some(source) = self.source.as_mut() else {
				self.outgoing.resize(samples, 0.0);
				break;
			};
			match source.next_packet().map_err(|error| SinkError::OnWrite(error.to_string()))? {
				Some((_, AudioPacket::Samples(packet))) => {
					self.packet = packet;
					self.cursor = 0;
				},
				Some((_, AudioPacket::Raw(_))) => {
					return Err(SinkError::InvalidParams("crossfade requires decoded stereo PCM".into()));
				},
				None => {
					self.source = None;
					self.packet.clear();
					self.cursor = 0;
				},
			}
		}
		Ok(())
	}

	fn fade_start(&mut self) {
		for frame in self.samples.as_chunks_mut::<2>().0.iter_mut().take(self.start_remaining) {
			let gain = 1.0 - self.start_remaining as f32 / CLICK_FADE_FRAMES as f32;
			for sample in frame {
				*sample *= gain;
			}
			self.start_remaining -= 1;
		}
	}
}

impl Sink for RingSink {
	fn new_track(&mut self) {
		// The equal-power crossfade already fades the incoming track from zero.
		self.start_remaining = if self.fade.remaining() == 0 { CLICK_FADE_FRAMES } else { 0 };
	}

	fn crossfade_duration(&self, current: &SpotifyUri, next: &SpotifyUri) -> u32 {
		if self.fade.remaining() > 0 {
			return 0;
		}
		let seconds = self.writer.crossfade();
		if seconds == 0 || !self.transitions.allows(current, next) { 0 } else { u32::from(seconds) * 1000 }
	}

	fn begin_crossfade(&mut self, outgoing: CrossfadeSource, duration_ms: u32) {
		self.source = Some(outgoing);
		self.packet.clear();
		self.cursor = 0;
		self.fade.start(usize::try_from(duration_ms).unwrap_or(0) * 44_100 / 1000);
		self.start_remaining = 0;
	}

	fn reset(&mut self) {
		self.source = None;
		self.packet.clear();
		self.cursor = 0;
		self.fade.cancel();
		self.start_remaining = CLICK_FADE_FRAMES;
		self.writer.clear();
	}

	fn write(&mut self, packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
		let AudioPacket::Samples(samples) = packet else {
			return Err(SinkError::InvalidParams("raw packets (passthrough) are not supported".into()));
		};
		// The decoder already owns interleaved render-format PCM. Move it instead of copying
		// and narrowing an entire packet into a second buffer.
		self.samples = samples;
		self.fade_start();
		if self.fade.remaining() > 0 {
			self.read_outgoing((self.samples.len() / 2).min(self.fade.remaining()) * 2)?;
			self.fade.mix(&mut self.samples, &self.outgoing);
			if self.fade.remaining() == 0 {
				self.source = None;
				self.packet.clear();
			}
		}
		if self.writer.write(&self.samples) { Ok(()) } else { Err(SinkError::OnWrite("the audio output is gone".into())) }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn track(byte: u128) -> SpotifyUri {
		SpotifyUri::from_uri(&format!("spotify:track:{byte:022}")).unwrap()
	}

	#[test]
	fn ordered_album_adjacency_is_gapless_but_shuffle_queue_and_wrap_may_fade() {
		let transitions = Transitions::default();
		let (first, second, third) = (track(1), track(2), track(3));
		transitions.context("spotify:album:example", false);
		assert!(!transitions.allows(&first, &second), "unknown album order is conservative");
		transitions.0.write().unwrap().album = vec![first.clone(), second.clone(), third.clone()];
		assert!(!transitions.allows(&first, &second));
		assert!(!transitions.allows(&second, &third), "including adjacency across discs");
		assert!(transitions.allows(&first, &third), "a queued nonadjacent song");
		assert!(transitions.allows(&third, &first), "album repeat is not a continuous join");
		transitions.context("spotify:album:example", true);
		assert!(transitions.allows(&first, &second));
		transitions.context("spotify:playlist:example", false);
		assert!(transitions.allows(&first, &second));
		transitions.context("", false);
		assert!(!transitions.allows(&first, &second));
		let episode = SpotifyUri::from_uri("spotify:episode:0000000000000000000001").unwrap();
		transitions.context("spotify:playlist:example", false);
		assert!(!transitions.allows(&first, &episode));
		assert!(!transitions.allows(&episode, &second));
	}

	#[test]
	fn a_user_skip_cancels_the_overlap_and_ramps_the_new_tracks_first_frames() {
		let (writer, _feed) = crate::audio::channel();
		let mut sink = RingSink::new(writer, Arc::new(Transitions::default()), Box::new(librespot_playback::mixer::NoOpVolume));
		let mut converter = Converter::new(None);
		sink.fade.start(441_000);
		sink.packet = vec![0.5; 1024];
		sink.write(AudioPacket::Samples(vec![1.0; 512]), &mut converter).unwrap();
		assert!(sink.fade.remaining() > 0);
		sink.reset();
		sink.write(AudioPacket::Samples(vec![0.75; 512]), &mut converter).unwrap();
		assert_eq!(sink.samples.first(), Some(&0.0));
		assert!(sink.samples.as_chunks::<2>().0.windows(2).all(|pair| pair[0][0] <= pair[1][0]));
		assert_eq!(sink.start_remaining, CLICK_FADE_FRAMES - 256);
		assert_eq!(sink.fade.remaining(), 0);
		assert!(sink.packet.is_empty() && sink.source.is_none());
	}

	#[test]
	fn track_start_ramp_is_stereo_and_sample_counted_across_packets() {
		let (writer, _feed) = crate::audio::channel();
		let mut sink = RingSink::new(writer, Arc::new(Transitions::default()), Box::new(librespot_playback::mixer::NoOpVolume));
		let mut converter = Converter::new(None);
		sink.write(AudioPacket::Samples(vec![0.5; 200]), &mut converter).unwrap();
		assert_eq!(&sink.samples[..2], &[0.0, 0.0]);
		assert!(sink.samples.as_chunks::<2>().0.iter().all(|frame| frame[0] == frame[1]));
		sink.write(AudioPacket::Samples(vec![0.5; CLICK_FADE_FRAMES * 2]), &mut converter).unwrap();
		assert!((sink.samples[0] - 0.5 * 100.0 / CLICK_FADE_FRAMES as f32).abs() < 1e-6);
		assert_eq!(sink.samples.last(), Some(&0.5));
		assert_eq!(sink.start_remaining, 0);
		sink.new_track();
		assert_eq!(sink.start_remaining, CLICK_FADE_FRAMES, "natural loads also arm the click ramp");
		sink.fade.start(4410);
		sink.new_track();
		assert_eq!(sink.start_remaining, 0, "do not attenuate the outgoing side of a crossfade");
	}

	#[test]
	fn render_format_packets_move_without_a_copy_or_changed_sample_bits() {
		let (writer, _feed) = crate::audio::channel();
		let mut sink = RingSink::new(writer, Arc::new(Transitions::default()), Box::new(librespot_playback::mixer::NoOpVolume));
		sink.start_remaining = 0;
		let samples = vec![-1.0f32, 1.0, -0.0, 0.0, f32::from_bits(1), -f32::MIN_POSITIVE, 0.123_456_7, -0.999_999_94];
		let pointer = samples.as_ptr();
		let previous: Vec<u32> = samples.iter().map(|&sample| dsp_f32(f64::from(sample)).to_bits()).collect();
		sink.write(AudioPacket::Samples(samples), &mut Converter::new(None)).unwrap();
		assert_eq!(sink.samples.as_ptr(), pointer, "RingSink must retain the decoder's allocation");
		assert_eq!(sink.samples.iter().map(|sample| sample.to_bits()).collect::<Vec<_>>(), previous);
	}
}
