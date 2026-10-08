//! Explicit context/list loads, including resuming a passive startup snapshot at its position.

use librespot_connect::{LoadContextOptions, LoadRequest, LoadRequestOptions, Options as ConnectOptions, PlayingTrack};

use crate::spotify::PlayStart;

pub(super) fn request(start: PlayStart, current: (bool, bool, bool)) -> LoadRequest {
	let (shuffle, repeat, repeat_track) = current;
	let options = |seek_to: u64, playing_track: Option<PlayingTrack>, context_options: ConnectOptions| LoadRequestOptions {
		start_playing: true,
		seek_to: u32::try_from(seek_to).unwrap_or(u32::MAX),
		context_options: Some(LoadContextOptions::Options(context_options)),
		playing_track,
	};
	let context_options = ConnectOptions { shuffle, repeat, repeat_track };
	match start {
		PlayStart::Context { uri, offset_uri, position_ms } => {
			LoadRequest::from_context_uri(uri, options(position_ms, offset_uri.map(PlayingTrack::Uri), context_options))
		},
		PlayStart::Tracks { uris, offset_uri } => {
			LoadRequest::from_tracks(uris, options(0, offset_uri.map(PlayingTrack::Uri), context_options))
		},
		// A rebuilt queue or a contextless restored track starts from its first song.
		PlayStart::List { uris, position_ms } => {
			LoadRequest::from_tracks(uris, options(position_ms, Some(PlayingTrack::Index(0)), ConnectOptions::default()))
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		Msg, State,
		engine::{Entity, LastPlayback, Options},
	};

	#[test]
	fn restored_context_toggle_reaches_the_load_position_and_track() {
		let mut item = Entity { uri: "spotify:track:last".into(), ..Entity::default() };
		item.decorations.playback.duration_ms = 248_000;
		let mut state = State::default();
		assert!(state.restore_playback(LastPlayback {
			item,
			context: Entity { uri: "spotify:playlist:context".into(), ..Entity::default() },
			position_ms: 2_000,
			options: Options::default(),
		}));
		let effects = state.update(Msg::TogglePlay);
		let [crate::app::Effect::Player(super::super::Command::Load(start))] = effects.as_slice() else {
			panic!("expected one explicit load")
		};
		assert_eq!(*start, PlayStart::Context {
			uri:         "spotify:playlist:context".into(),
			offset_uri:  Some("spotify:track:last".into()),
			position_ms: 2_000,
		});
		let load = request(start.clone(), (false, false, false));
		assert!(load.start_playing);
		assert_eq!(load.seek_to, 2_000);
		assert!(matches!(&load.playing_track, Some(PlayingTrack::Uri(uri)) if uri == "spotify:track:last"));
		assert!(!state.active, "only executing the user command may activate the player");
	}

	#[test]
	fn contextless_resume_does_not_lose_the_saved_position() {
		let load = request(PlayStart::List { uris: vec!["spotify:track:last".into()], position_ms: 17_000 }, (true, true, true));
		assert!(load.start_playing);
		assert_eq!(load.seek_to, 17_000);
		assert!(matches!(load.playing_track, Some(PlayingTrack::Index(0))));
	}
}
