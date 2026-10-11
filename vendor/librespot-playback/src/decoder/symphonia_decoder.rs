use std::{io, time::Duration};

use symphonia::core::{
    audio::{AudioBuffer, AudioBufferRef, Signal},
    codecs::{Decoder, DecoderOptions},
    conv::IntoSample,
    errors::Error,
    formats::{FormatOptions, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions},
    meta::{MetadataOptions, StandardTagKey, Value},
    probe::{Hint, ProbeResult},
    sample::Sample,
};

use super::{AudioDecoder, AudioPacket, AudioPacketPosition, DecoderError, DecoderResult};

use crate::{NUM_CHANNELS, PAGES_PER_MS, SAMPLE_RATE, player::NormalisationData, symphonia_util};

pub struct SymphoniaDecoder {
    probe_result: ProbeResult,
    decoder: Box<dyn Decoder>,
}

// The sink owns each packet, so interleave directly into that allocation. Keeping an
// extra SampleBuffer would duplicate every decoded packet and retain unused scratch.
fn interleave_stereo<S: Sample + IntoSample<f32>>(decoded: &AudioBuffer<S>) -> Vec<f32> {
    decoded.chan(0).iter().zip(decoded.chan(1))
        .flat_map(|(&left, &right)| [left.into_sample(), right.into_sample()])
        .collect()
}

fn pcm_packet(decoded: AudioBufferRef<'_>) -> Vec<f32> {
    match decoded {
        AudioBufferRef::U8(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::U16(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::U24(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::U32(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::S8(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::S16(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::S24(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::S32(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::F32(buffer) => interleave_stereo(&buffer),
        AudioBufferRef::F64(buffer) => interleave_stereo(&buffer),
    }
}

#[derive(Default)]
pub(crate) struct LocalFileMetadata {
    pub name: Option<String>,
    pub language: Option<String>,
    pub album: Option<String>,
    pub artists: Option<String>,
    pub album_artists: Option<String>,
    pub number: Option<u32>,
    pub disc_number: Option<u32>,
}

impl SymphoniaDecoder {
    pub fn new<R>(input: R, hint: Hint) -> DecoderResult<Self>
    where
        R: MediaSource + 'static,
    {
        let mss_opts = MediaSourceStreamOptions {
            buffer_len: librespot_audio::AudioFetchParams::get().minimum_download_size,
        };
        let mss = MediaSourceStream::new(Box::new(input), mss_opts);

        let format_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let metadata_opts: MetadataOptions = Default::default();

        let probe_result =
            symphonia::default::get_probe().format(&hint, mss, &format_opts, &metadata_opts)?;

        let format = &probe_result.format;

        let track = format.default_track().ok_or_else(|| {
            DecoderError::SymphoniaDecoder("Could not retrieve default track".into())
        })?;

        let decoder_opts: DecoderOptions = Default::default();

        let decoder = symphonia::default::get_codecs().make(&track.codec_params, &decoder_opts)?;

        let rate = decoder.codec_params().sample_rate.ok_or_else(|| {
            DecoderError::SymphoniaDecoder("Could not retrieve sample rate".into())
        })?;

        // TODO: The official client supports local files with sample rates other than 44,100 kHz.
        // To play these accurately, we need to either resample the input audio, or introduce a way
        // to change the player's current sample rate (likely by closing and re-opening the sink
        // with new parameters).
        if rate != SAMPLE_RATE {
            return Err(DecoderError::SymphoniaDecoder(format!(
                "Unsupported sample rate: {rate}"
            )));
        }

        let channels = decoder.codec_params().channels.ok_or_else(|| {
            DecoderError::SymphoniaDecoder("Could not retrieve channel configuration".into())
        })?;
        if channels.count() != NUM_CHANNELS as usize {
            return Err(DecoderError::SymphoniaDecoder(format!(
                "Unsupported number of channels: {channels}"
            )));
        }

        Ok(Self {
            probe_result,
            decoder,
        })
    }

    pub fn normalisation_data(&mut self) -> Option<NormalisationData> {
        let metadata = symphonia_util::get_latest_metadata(&mut self.probe_result)?;
        let tags = metadata.current()?.tags();

        if tags.is_empty() {
            None
        } else {
            let mut data = NormalisationData::default();

            for tag in tags {
                if let Value::Float(value) = tag.value {
                    match tag.std_key {
                        Some(StandardTagKey::ReplayGainAlbumGain) => data.album_gain_db = value,
                        Some(StandardTagKey::ReplayGainAlbumPeak) => data.album_peak = value,
                        Some(StandardTagKey::ReplayGainTrackGain) => data.track_gain_db = value,
                        Some(StandardTagKey::ReplayGainTrackPeak) => data.track_peak = value,
                        _ => (),
                    }
                }
            }

            Some(data)
        }
    }

    pub(crate) fn local_file_metadata(&mut self) -> Option<LocalFileMetadata> {
        let metadata = symphonia_util::get_latest_metadata(&mut self.probe_result)?;
        let tags = metadata.current()?.tags();
        let mut metadata = LocalFileMetadata::default();

        for tag in tags {
            if let Value::String(value) = &tag.value {
                match tag.std_key {
                    // We could possibly use mem::take here to avoid cloning, but that risks leaving
                    // the audio item metadata in a bad state.
                    Some(StandardTagKey::TrackTitle) => metadata.name = Some(value.clone()),
                    Some(StandardTagKey::Language) => metadata.language = Some(value.clone()),
                    Some(StandardTagKey::Artist) => metadata.artists = Some(value.clone()),
                    Some(StandardTagKey::AlbumArtist) => {
                        metadata.album_artists = Some(value.clone())
                    }
                    Some(StandardTagKey::Album) => metadata.album = Some(value.clone()),
                    Some(StandardTagKey::TrackNumber) => {
                        metadata.number = match value.parse::<u32>() {
                            Ok(value) => Some(value),
                            Err(e) => {
                                warn!(
                                    "Failed to parse local file's track number tag '{value}': {e}"
                                );
                                None
                            }
                        }
                    }
                    Some(StandardTagKey::DiscNumber) => {
                        metadata.disc_number = match value.parse::<u32>() {
                            Ok(value) => Some(value),
                            Err(e) => {
                                warn!(
                                    "Failed to parse local file's disc number tag '{value}': {e}"
                                );
                                None
                            }
                        }
                    }
                    _ => (),
                }
            } else if let Value::UnsignedInt(value) = &tag.value {
                match tag.std_key {
                    Some(StandardTagKey::TrackNumber) => metadata.number = Some(*value as u32),
                    Some(StandardTagKey::DiscNumber) => metadata.disc_number = Some(*value as u32),
                    _ => (),
                }
            } else if let Value::SignedInt(value) = &tag.value {
                match tag.std_key {
                    Some(StandardTagKey::TrackNumber) => metadata.number = Some(*value as u32),
                    Some(StandardTagKey::DiscNumber) => metadata.disc_number = Some(*value as u32),
                    _ => (),
                }
            }
        }

        Some(metadata)
    }

    #[inline]
    fn ts_to_ms(&self, ts: u64) -> u32 {
        match self.decoder.codec_params().time_base {
            Some(time_base) => {
                let time = Duration::from(time_base.calc_time(ts));
                time.as_millis() as u32
            }
            // Fallback in the unexpected case that the format has no base time set.
            None => (ts as f64 * PAGES_PER_MS) as u32,
        }
    }
}

impl AudioDecoder for SymphoniaDecoder {
    fn seek(&mut self, position_ms: u32) -> Result<u32, DecoderError> {
        // "Saturate" the position_ms to the duration of the track if it exceeds it.
        let mut target = Duration::from_millis(position_ms.into());
        let codec_params = self.decoder.codec_params();
        if let (Some(time_base), Some(n_frames)) = (codec_params.time_base, codec_params.n_frames) {
            let duration = Duration::from(time_base.calc_time(n_frames));
            if target > duration {
                target = duration;
            }
        }

        // `track_id: None` implies the default track ID (of the container, not of Spotify).
        let seeked_to_ts = self.probe_result.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: target.into(),
                track_id: None,
            },
        )?;

        // Seeking is a `FormatReader` operation, so the decoder cannot reliably
        // know when a seek took place. Reset it to avoid audio glitches.
        self.decoder.reset();

        Ok(self.ts_to_ms(seeked_to_ts.actual_ts))
    }

    fn next_packet(&mut self) -> DecoderResult<Option<(AudioPacketPosition, AudioPacket)>> {
        let mut skipped = false;

        loop {
            let packet = match self.probe_result.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(err)) => {
                    if err.kind() == io::ErrorKind::UnexpectedEof {
                        return Ok(None);
                    } else {
                        return Err(DecoderError::SymphoniaDecoder(err.to_string()));
                    }
                }
                Err(err) => {
                    return Err(err.into());
                }
            };

            let position_ms = self.ts_to_ms(packet.ts());
            let packet_position = AudioPacketPosition {
                position_ms,
                skipped,
            };

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let samples = AudioPacket::Samples(pcm_packet(decoded));

                    return Ok(Some((packet_position, samples)));
                }
                Err(Error::DecodeError(_)) => {
                    // The packet failed to decode due to corrupted or invalid data, get a new
                    // packet and try again.
                    warn!("Skipping malformed audio packet at {position_ms} ms");
                    skipped = true;
                    continue;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use symphonia::core::{
        audio::{AudioBuffer, Layout, SampleBuffer, Signal, SignalSpec},
        conv::IntoSample,
        sample::Sample,
    };

    use super::interleave_stereo;

    fn compare<S: Sample + IntoSample<f32> + IntoSample<f64>>(values: &[S]) {
        let spec = SignalSpec::new_with_layout(44_100, Layout::Stereo);
        let mut decoded = AudioBuffer::<S>::new(values.len() as u64, spec);
        decoded.render_reserved(None);
        decoded.chan_mut(0).copy_from_slice(values);
        for (sample, value) in decoded.chan_mut(1).iter_mut().zip(values.iter().rev()) {
            *sample = *value;
        }
        let native = interleave_stereo(&decoded);
        let mut previous = SampleBuffer::<f64>::new(values.len() as u64, spec);
        previous.copy_interleaved_typed(&decoded);
        for (&actual, &reference) in native.iter().zip(previous.samples()) {
            assert_eq!(actual.to_bits(), (reference as f32).to_bits());
            for volume in [1.0, 0.5, 0.1, 0.001] {
                let actual = (f64::from(actual) * volume) as f32;
                let reference = (reference * volume) as f32;
                // 32-bit lossless PCM can round before attenuation; never more than one f32 ulp.
                assert!((actual - reference).abs() <= f32::EPSILON);
            }
        }
    }

    #[test]
    fn native_pcm_matches_widen_then_narrow_for_lossy_and_lossless_samples() {
        compare(&[i16::MIN, -32767, -1, 0, 1, 32766, i16::MAX]);
        compare(&[i32::MIN, i32::MIN + 1, -257, -256, -1, 0, 1, 256, 257, i32::MAX - 1, i32::MAX]);
        compare(&[-1.0f32, -0.25, -0.0, 0.0, f32::from_bits(1), f32::MIN_POSITIVE, 0.125, 0.999_999_94, 1.0]);
        let mut state = 37u32;
        let integers: Vec<i32> = (0..8192).map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            i32::from_ne_bytes(state.to_ne_bytes())
        }).collect();
        compare(&integers);
        let floats: Vec<f32> = integers.iter().map(|&value| value as f32 / 2_147_483_648.0).collect();
        compare(&floats);
    }
}
