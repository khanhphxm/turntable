use std::{sync::Arc, time::Instant};

use windows::{
	Foundation::{TimeSpan, TypedEventHandler},
	Media::{
		MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs, SystemMediaTransportControls,
		SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs, SystemMediaTransportControlsDisplayUpdater,
		SystemMediaTransportControlsTimelineProperties,
	},
	Storage::Streams::{IRandomAccessStream, RandomAccessStreamReference},
	Win32::{
		Foundation::{E_INVALIDARG, RPC_E_CHANGED_MODE},
		System::WinRT::{
			BSOS_DEFAULT, CreateRandomAccessStreamOverStream, ISystemMediaTransportControlsInterop, RO_INIT_SINGLETHREADED, RoInitialize,
			RoUninitialize,
		},
		UI::{Input::KeyboardAndMouse::GetActiveWindow, Shell::SHCreateMemStream},
	},
	core::{HSTRING, factory},
};

use super::{Command, Metadata, Playback};

// COM/WinRT apartment registration must be balanced on the constructing thread.
struct Apartment(bool);

impl Apartment {
	fn new() -> Result<Self, String> {
		// SAFETY: initializes this calling GUI thread, never another thread. If
		// GPUI already initialized an MTA, use it rather than changing its model.
		match unsafe { RoInitialize(RO_INIT_SINGLETHREADED) } {
			Ok(()) => Ok(Self(true)),
			Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self(false)),
			Err(error) => Err(error.to_string()),
		}
	}
}

impl Drop for Apartment {
	fn drop(&mut self) {
		if self.0 {
			// SAFETY: this balances a successful RoInitialize on the same thread.
			unsafe {
				RoUninitialize();
			}
		}
	}
}

pub(super) struct Backend {
	controls:       SystemMediaTransportControls,
	display:        SystemMediaTransportControlsDisplayUpdater,
	timeline:       SystemMediaTransportControlsTimelineProperties,
	button_token:   Option<i64>,
	position_token: Option<i64>,
	artwork_bytes:  Option<Arc<[u8]>>,
	duration_ms:    u64,
	playback:       Playback,
	updated_at:     Instant,
	// Declared last: release COM objects before uninitializing this apartment.
	_apartment:     Apartment,
}

impl Backend {
	pub(super) fn new(on_command: Arc<dyn Fn(Command) + Send + Sync>) -> Result<Self, String> {
		let apartment = Apartment::new()?;
		// SAFETY: GetActiveWindow returns only a window belonging to this GUI
		// thread; never attach our media session to another app's foreground HWND.
		let window = unsafe { GetActiveWindow() };
		if window.0.is_null() {
			return Err("Create MediaControls on the GPUI thread after activating its window".into());
		}
		let interop = factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>().map_err(|error| error.to_string())?;
		// SAFETY: this is the live window of the calling GPUI thread. The desktop
		// interop is supported without UWP/CoreWindow or a dummy audio player.
		let controls: SystemMediaTransportControls = unsafe { interop.GetForWindow(window) }.map_err(|error| error.to_string())?;
		let display = controls.DisplayUpdater().map_err(|error| error.to_string())?;
		let timeline = SystemMediaTransportControlsTimelineProperties::new().map_err(|error| error.to_string())?;
		let mut backend = Self {
			controls,
			display,
			timeline,
			button_token: None,
			position_token: None,
			artwork_bytes: None,
			duration_ms: 0,
			playback: Playback::Stopped,
			updated_at: Instant::now(),
			_apartment: apartment,
		};
		backend.initialize(on_command).map_err(|error| error.to_string())?;
		Ok(backend)
	}

	fn initialize(&mut self, on_command: Arc<dyn Fn(Command) + Send + Sync>) -> windows::core::Result<()> {
		self.controls.SetIsPlayEnabled(true)?;
		self.controls.SetIsPauseEnabled(true)?;
		self.controls.SetIsNextEnabled(true)?;
		self.controls.SetIsPreviousEnabled(true)?;
		self.controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped)?;
		self.controls.SetIsEnabled(false)?;
		self.controls.SetPlaybackRate(1.0)?;
		self.display.SetType(MediaPlaybackType::Music)?;
		self.timeline.SetStartTime(timespan(0))?;
		self.timeline.SetMinSeekTime(timespan(0))?;
		let callback = Arc::clone(&on_command);
		self.button_token = Some(self.controls.ButtonPressed(&TypedEventHandler::new(
			move |_, event: windows::core::Ref<'_, SystemMediaTransportControlsButtonPressedEventArgs>| {
				if let Some(event) = event.as_ref() {
					let command = match event.Button()? {
						SystemMediaTransportControlsButton::Play => Some(Command::Play),
						SystemMediaTransportControlsButton::Pause => Some(Command::Pause),
						SystemMediaTransportControlsButton::Next => Some(Command::Next),
						SystemMediaTransportControlsButton::Previous => Some(Command::Previous),
						_ => None,
					};
					if let Some(command) = command {
						callback(command);
					}
				}
				Ok(())
			},
		))?);
		self.position_token = Some(self.controls.PlaybackPositionChangeRequested(&TypedEventHandler::new(
			move |_, event: windows::core::Ref<'_, PlaybackPositionChangeRequestedEventArgs>| {
				if let Some(event) = event.as_ref()
					&& let Ok(ticks) = u64::try_from(event.RequestedPlaybackPosition()?.Duration)
				{
					on_command(Command::SetPosition(ticks / 10_000));
				}
				Ok(())
			},
		))?);
		Ok(())
	}

	pub(super) fn set_metadata(&mut self, metadata: &Metadata) -> Result<(), String> {
		self.update_metadata(metadata).map_err(|error| error.to_string())
	}

	fn update_metadata(&mut self, metadata: &Metadata) -> windows::core::Result<()> {
		let artwork_changed = self.artwork_bytes != metadata.artwork;
		let artwork = if artwork_changed { metadata.artwork.as_ref().map(|bytes| thumbnail(bytes)).transpose()? } else { None };
		let music = self.display.MusicProperties()?;
		music.SetTitle(&HSTRING::from(&metadata.title))?;
		music.SetArtist(&HSTRING::from(&metadata.artist))?;
		music.SetAlbumTitle(&HSTRING::from(&metadata.album))?;
		if artwork_changed {
			self.display.SetThumbnail(artwork.as_ref())?;
			self.artwork_bytes = metadata.artwork.clone();
		}
		self.duration_ms = metadata.duration_ms;
		if !matches!(self.playback, Playback::Stopped) {
			self.display.Update()?;
		}
		self.update_timeline()
	}

	pub(super) fn set_playback(&mut self, playback: Playback) -> Result<(), String> {
		self.update_playback(playback).map_err(|error| error.to_string())
	}

	fn update_playback(&mut self, playback: Playback) -> windows::core::Result<()> {
		let was_stopped = matches!(self.playback, Playback::Stopped);
		self.playback = playback;
		self.updated_at = Instant::now();
		if matches!(playback, Playback::Stopped) {
			self.controls.SetPlaybackStatus(MediaPlaybackStatus::Stopped)?;
			// Disable the session to remove it from the system picker, retaining
			// cached metadata so restarting does not require another artwork decode.
			self.controls.SetIsEnabled(false)?;
		} else {
			if was_stopped {
				self.display.Update()?;
			}
			self.controls.SetIsEnabled(true)?;
			self.controls.SetPlaybackStatus(if matches!(playback, Playback::Playing { .. }) {
				MediaPlaybackStatus::Playing
			} else {
				MediaPlaybackStatus::Paused
			})?;
		}
		self.update_timeline()
	}

	fn update_timeline(&self) -> windows::core::Result<()> {
		let elapsed = if matches!(self.playback, Playback::Playing { .. }) {
			u64::try_from(self.updated_at.elapsed().as_millis()).unwrap_or(u64::MAX)
		} else {
			0
		};
		let position = self.playback.position_ms().saturating_add(elapsed).min(self.duration_ms);
		self.timeline.SetEndTime(timespan(self.duration_ms))?;
		self.timeline.SetMaxSeekTime(timespan(self.duration_ms))?;
		self.timeline.SetPosition(timespan(position))?;
		self.controls.UpdateTimelineProperties(&self.timeline)
	}
}

impl Drop for Backend {
	fn drop(&mut self) {
		if let Some(token) = self.button_token.take() {
			let _ = self.controls.RemoveButtonPressed(token);
		}
		if let Some(token) = self.position_token.take() {
			let _ = self.controls.RemovePlaybackPositionChangeRequested(token);
		}
		let _ = self.controls.SetPlaybackStatus(MediaPlaybackStatus::Closed);
		let _ = self.controls.SetIsEnabled(false);
		let _ = self.display.ClearAll();
		let _ = self.display.Update();
	}
}

fn timespan(milliseconds: u64) -> TimeSpan {
	TimeSpan { Duration: i64::try_from(milliseconds.saturating_mul(10_000)).unwrap_or(i64::MAX) }
}

fn thumbnail(bytes: &[u8]) -> windows::core::Result<RandomAccessStreamReference> {
	// The generated SHCreateMemStream wrapper converts length to u32 internally.
	// Check once before calling it to avoid the wrapper's length overflow panic.
	u32::try_from(bytes.len())
		.map_err(|error| windows::core::Error::new(E_INVALIDARG, format!("Artwork is larger than Windows supports: {error}")))?;
	// SAFETY: SHCreateMemStream copies this valid byte slice into an owned COM
	// stream. The WinRT bridge and thumbnail retain it; no temporary files,
	// async wait on an STA, or second encoded-byte copy are needed.
	let stream = unsafe { SHCreateMemStream(Some(bytes)) }.ok_or_else(windows::core::Error::from_win32)?;
	// SAFETY: this live, owned IStream supports the interfaces required by the
	// bridge; the resulting IRandomAccessStream retains it.
	let stream: IRandomAccessStream = unsafe { CreateRandomAccessStreamOverStream(&stream, BSOS_DEFAULT) }?;
	RandomAccessStreamReference::CreateFromStream(&stream)
}

#[cfg(test)]
mod tests {
	use super::timespan;

	#[test]
	fn timeline_conversion_saturates() {
		assert_eq!(timespan(12_345).Duration, 123_450_000);
		assert_eq!(timespan(u64::MAX).Duration, i64::MAX);
	}
}
