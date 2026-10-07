use std::{
	ffi::c_void,
	ptr::NonNull,
	sync::Arc,
	time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::{AnyThread as _, MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::NSImage;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
	MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle, MPMediaItemPropertyArtist,
	MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration, MPMediaItemPropertyTitle, MPNowPlayingInfoCenter,
	MPNowPlayingInfoPropertyDefaultPlaybackRate, MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyPlaybackRate,
	MPNowPlayingPlaybackState, MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};

use super::{Command, Metadata, Playback};

struct Target {
	command: Retained<MPRemoteCommand>,
	token:   Retained<AnyObject>,
}

impl Drop for Target {
	fn drop(&mut self) {
		// SAFETY: this token was returned by this command's addTargetWithHandler.
		unsafe {
			self.command.removeTarget(Some(&self.token));
			self.command.setEnabled(false);
		}
	}
}

pub(super) struct Backend {
	center:        Retained<MPNowPlayingInfoCenter>,
	info:          Retained<NSMutableDictionary<NSString, AnyObject>>,
	_targets:      [Target; 6],
	artwork_bytes: Option<Arc<[u8]>>,
	playback:      Playback,
	updated_at:    Instant,
	duration_ms:   u64,
	_main_thread:  MainThreadMarker,
}

impl Backend {
	pub(super) fn new(on_command: Arc<dyn Fn(Command) + Send + Sync>) -> Result<Self, String> {
		let main_thread = MainThreadMarker::new().ok_or("MediaControls must be created on the macOS main thread")?;
		// SAFETY: framework singletons are valid on the main thread; GPUI owns the
		// Cocoa run loop which delivers MPRemoteCommandCenter events.
		let (center, commands) = unsafe { (MPNowPlayingInfoCenter::defaultCenter(), MPRemoteCommandCenter::sharedCommandCenter()) };
		// SAFETY: these getters and command registrations use framework-defined
		// event types; each block is copied by the command center.
		let targets = unsafe {
			let [play, pause, toggle, next, previous] = [
				(commands.playCommand(), Command::Play),
				(commands.pauseCommand(), Command::Pause),
				(commands.togglePlayPauseCommand(), Command::Toggle),
				(commands.nextTrackCommand(), Command::Next),
				(commands.previousTrackCommand(), Command::Previous),
			]
			.map(|(native, command)| {
				let callback = Arc::clone(&on_command);
				let handler = RcBlock::new(move |_: NonNull<MPRemoteCommandEvent>| {
					callback(command);
					MPRemoteCommandHandlerStatus::Success
				});
				let token = native.addTargetWithHandler(&handler);
				native.setEnabled(true);
				Target { command: native, token }
			});
			let native = commands.changePlaybackPositionCommand();
			let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
				let event = event.as_ref();
				if let Some(event) = event.downcast_ref::<MPChangePlaybackPositionCommandEvent>()
					&& let Ok(position) = Duration::try_from_secs_f64(event.positionTime())
				{
					let position_ms = u64::try_from(position.as_millis()).unwrap_or(u64::MAX);
					on_command(Command::SetPosition(position_ms));
					return MPRemoteCommandHandlerStatus::Success;
				}
				MPRemoteCommandHandlerStatus::CommandFailed
			});
			let token = native.addTargetWithHandler(&handler);
			native.setEnabled(true);
			[play, pause, toggle, next, previous, Target { command: native.into_super(), token }]
		};
		Ok(Self {
			center,
			info: NSMutableDictionary::new(),
			_targets: targets,
			artwork_bytes: None,
			playback: Playback::Stopped,
			updated_at: Instant::now(),
			duration_ms: 0,
			_main_thread: main_thread,
		})
	}

	pub(super) fn set_metadata(&mut self, metadata: &Metadata) -> Result<(), String> {
		let artwork_changed = self.artwork_bytes != metadata.artwork;
		let artwork =
			if artwork_changed { metadata.artwork.as_ref().map(|bytes| decode_artwork(Arc::clone(bytes))).transpose()? } else { None };
		// SAFETY: exported MediaPlayer keys are immutable NSStrings. Dictionary
		// values have the documented NSString, NSNumber, and artwork types.
		unsafe {
			self.info.insert(MPMediaItemPropertyTitle, &NSString::from_str(&metadata.title));
			self.info.insert(MPMediaItemPropertyArtist, &NSString::from_str(&metadata.artist));
			self.info.insert(MPMediaItemPropertyAlbumTitle, &NSString::from_str(&metadata.album));
			self.info.insert(MPMediaItemPropertyPlaybackDuration, &NSNumber::numberWithDouble(metadata.duration_ms as f64 / 1_000.0));
			if artwork_changed {
				if let Some(artwork) = artwork {
					self.info.insert(MPMediaItemPropertyArtwork, &artwork);
				} else {
					self.info.removeObjectForKey(MPMediaItemPropertyArtwork);
				}
			}
		}
		if artwork_changed {
			self.artwork_bytes = metadata.artwork.clone();
		}
		self.duration_ms = metadata.duration_ms;
		self.publish();
		Ok(())
	}

	#[expect(clippy::unnecessary_wraps, reason = "All platform backends expose the same fallible update API")]
	pub(super) fn set_playback(&mut self, playback: Playback) -> Result<(), String> {
		self.playback = playback;
		self.updated_at = Instant::now();
		self.publish();
		Ok(())
	}

	fn publish(&self) {
		let (state, rate) = match self.playback {
			Playback::Playing { .. } => (MPNowPlayingPlaybackState::Playing, 1.0),
			Playback::Paused { .. } => (MPNowPlayingPlaybackState::Paused, 0.0),
			Playback::Stopped => (MPNowPlayingPlaybackState::Stopped, 0.0),
		};
		let elapsed = if rate == 1.0 { self.updated_at.elapsed().as_secs_f64() } else { 0.0 };
		let mut position = self.playback.position_ms() as f64 / 1_000.0 + elapsed;
		if self.duration_ms > 0 {
			position = position.min(self.duration_ms as f64 / 1_000.0);
		}
		// SAFETY: all calls occur on the creating main thread. Setting the explicit
		// macOS playbackState is essential for media-key/session arbitration.
		unsafe {
			self.info.insert(MPNowPlayingInfoPropertyElapsedPlaybackTime, &NSNumber::numberWithDouble(position));
			self.info.insert(MPNowPlayingInfoPropertyPlaybackRate, &NSNumber::numberWithDouble(rate));
			self.info.insert(MPNowPlayingInfoPropertyDefaultPlaybackRate, &NSNumber::numberWithDouble(1.0));
			self.center.setNowPlayingInfo(if matches!(self.playback, Playback::Stopped) { None } else { Some(&self.info) });
			self.center.setPlaybackState(state);
		}
	}
}

impl Drop for Backend {
	fn drop(&mut self) {
		// SAFETY: this main-thread-owned session is ending; clearing the center
		// releases artwork and relinquishes native now-playing ownership.
		unsafe {
			self.center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
			self.center.setNowPlayingInfo(None);
		}
	}
}

fn decode_artwork(bytes: Arc<[u8]>) -> Result<Retained<MPMediaItemArtwork>, String> {
	let pointer = NonNull::new(bytes.as_ptr().cast_mut().cast::<c_void>()).ok_or("Artwork has no byte storage")?;
	let length = bytes.len();
	// NSData copies the deallocator block, retaining the Arc until its last byte
	// reader goes away. The block's destruction releases the Rust allocation;
	// Cocoa must not free or mutate that allocation. No encoded-byte copy.
	let deallocator = RcBlock::new(move |_: NonNull<c_void>, _: usize| {
		let _keep_alive = &bytes;
	});
	// SAFETY: immutable NSData borrows exactly length valid bytes. The copied
	// block owns their Arc, so lazy NSImage decoding cannot outlive the bytes.
	let data = unsafe { NSData::initWithBytesNoCopy_length_deallocator(NSData::alloc(), pointer, length, Some(&deallocator)) };
	let image = NSImage::initWithData(NSImage::alloc(), &data).ok_or("Cannot decode now-playing artwork")?;
	let size = image.size();
	let handler = RcBlock::new(move |_| NonNull::from(&*image));
	// SAFETY: the artwork copies the block which owns its immutable NSImage;
	// its return pointer is valid for the entire artwork/request lifetime.
	Ok(unsafe { MPMediaItemArtwork::initWithBoundsSize_requestHandler(MPMediaItemArtwork::alloc(), size, &handler) })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rejects_non_main_thread() -> Result<(), String> {
		let rejected =
			std::thread::spawn(|| Backend::new(Arc::new(|_| {})).is_err()).join().map_err(|_panic| "Thread-ownership check failed")?;
		assert!(rejected);
		Ok(())
	}
}
