//! UI message dispatch: optimistic state changes and the effects the player must carry out.

use std::time::Instant;

use super::{Dialog, Effect, Field, Msg, Panel, State, eq_step_db};
use crate::{
	engine::{Command, Repeat},
	library::{self, Route},
};

impl State {
	/// Handles every UI message once; action helpers keep their state and effects together.
	#[expect(clippy::too_many_lines, reason = "one exhaustive arm per UI message; nontrivial actions are delegated to small helpers")]
	pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
		if let Msg::Prefetch(route) = msg {
			return if route != self.library.route && !matches!(route, Route::Search | Route::Library | Route::Folder(_)) {
				vec![Effect::Library(library::Request::Prefetch(route))]
			} else {
				Vec::new()
			};
		}
		self.close_menus(&msg);
		if let Some(effects) = self.remote_update(&msg) {
			return effects;
		}
		let mut cmds = Vec::new();
		let mut effects = Vec::new();
		match msg {
			Msg::TogglePlay => self.toggle_play(&mut cmds),
			Msg::Play => {
				if !self.playing() {
					self.toggle_play(&mut cmds);
				}
			},
			Msg::Pause => cmds.push(Command::Pause),
			Msg::Next => self.activate_command(Command::SkipNext, &mut cmds),
			Msg::Prev => self.activate_command(Command::SkipPrev, &mut cmds),
			Msg::Seek(position_ms) => self.seek_absolute(position_ms, &mut cmds),
			Msg::SeekBy(delta) => self.seek_by(delta, &mut cmds),
			Msg::Volume(percent) => cmds.push(self.set_volume(percent.min(100))),
			Msg::ToggleMute => cmds.push(self.toggle_mute()),
			Msg::ToggleShuffle => {
				self.options.shuffle = !self.options.shuffle;
				cmds.push(Command::SetShuffle { enabled: self.options.shuffle });
			},
			Msg::CycleRepeat => self.cycle_repeat(&mut cmds),
			Msg::DevicesMenu => effects.extend(self.toggle_devices_menu()),
			Msg::PickDevice(id) => effects.extend(self.pick_device(&id)),
			Msg::TogglePanel(panel) => self.toggle_panel(panel, &mut cmds, &mut effects),
			Msg::RetryLyrics => {
				self.lyrics = None;
				self.lyrics_retry_at = None;
				self.lyrics_failures = 0;
			},
			Msg::PlayContext(uri) => {
				effects.push(Effect::Player(Command::Load(crate::spotify::PlayStart::Context { uri, offset_uri: None, position_ms: 0 })));
			},
			Msg::ToggleSidebar => {
				self.sidebar_collapsed = !self.sidebar_collapsed;
				effects.push(Effect::SaveSettings);
			},
			Msg::LibraryKind(kind) => self.toggle_library_kind(kind),
			Msg::LibrarySort(sort) => {
				self.library.sort = sort;
				self.library_menu = false;
			},
			Msg::LibraryView(view) => self.library.view = view,
			Msg::LibraryMenu => self.library_menu = !self.library_menu,
			Msg::ClearLibraryFilter => self.clear_library_filter(),
			Msg::ToggleShelf(id) => self.toggle_shelf(id),
			Msg::OpenUrl(url) => effects.push(Effect::OpenUrl(url)),
			Msg::EqEnabled(on) => {
				self.eq.enabled = on;
				effects.push(Effect::SetEq);
			},
			Msg::EqBand(band, step) => self.eq_set(band, eq_step_db(step), &mut effects),
			Msg::EqSet(band, db) => self.eq_set(band, crate::audio::sanitize_gain_db(db), &mut effects),
			Msg::EqPreset(name) => self.eq_preset(name, &mut effects),
			Msg::Navigate(route) => self.navigate(route, &mut effects),
			Msg::Prefetch(_) => {},
			Msg::RememberSearch(tile) => {
				self.remember_search(tile);
				effects.push(Effect::SaveRecentSearches);
			},
			Msg::ForgetSearch(route) => {
				self.forget_search(&route);
				effects.push(Effect::SaveRecentSearches);
			},
			Msg::Select(row) => {
				if let Some(i) = self.library.resolve_row(&row) {
					self.library.select(i);
				}
			},
			Msg::PlayRow(row) => {
				if let Some(i) = self.library.resolve_row(&row) {
					self.play_row(i, &mut effects);
				}
			},
			Msg::OpenTrack { album, uri } => {
				effects.extend(self.update(Msg::Navigate(Route::Album(album))));
				if let Some(index) = self.library.rows().iter().position(|track| track.uri == uri) {
					self.library.select(index);
					self.library.reveal = None;
				} else {
					self.library.reveal = Some(uri);
				}
			},
			Msg::ToggleFollow => self.toggle_follow(&mut effects),
			Msg::FollowArtist(id, name) => self.follow_artist(&id, name, &mut effects),
			Msg::CreatePlaylist => effects.push(Effect::Library(library::Request::CreatePlaylist(self.new_playlist_name(), Vec::new()))),
			Msg::CreateFolder => effects.push(Effect::Library(library::Request::CreateFolder)),
			Msg::StartRenameFolder(id) => self.start_rename_folder(id),
			Msg::RenameFolder(id, name) => {
				if let Some(name) = nonempty_text(name) {
					self.dialog = None;
					self.focus = None;
					effects.push(Effect::Library(library::Request::RenameFolder(id, name)));
				}
			},
			Msg::DeleteFolder(id) => effects.push(Effect::Library(library::Request::DeleteFolder(id))),
			Msg::MoveToFolder { uri, folder } => effects.push(Effect::Library(library::Request::MoveToFolder { uri, folder })),
			Msg::ToggleFolder(id) => effects.extend(self.library.toggle_folder(id).map(Effect::Library)),
			Msg::EditPlaylist(id) => effects.extend(self.edit_playlist(id).map(Effect::Library)),
			Msg::DeletePlaylist(id) => self.set_library_saved(format!("spotify:playlist:{id}"), false, &mut effects),
			Msg::SetPlaylistPublic(id, public) => effects.extend(self.set_playlist_public(id, public).map(Effect::Library)),
			Msg::SetCollaborative(id, collaborative) => {
				effects.extend(self.set_playlist_collaborative(id, collaborative).map(Effect::Library));
			},
			Msg::SetSaved { uri, saved } => self.set_library_saved(uri, saved, &mut effects),
			Msg::SetPinned { uri, pinned } => self.set_library_pinned(uri, pinned, &mut effects),
			Msg::ExcludeFromTaste { uri, excluded } => {
				if self.library.set_excluded(&uri, excluded) {
					effects.push(Effect::Library(library::Request::SetExcluded { uri, excluded }));
				}
			},
			Msg::AddContextToPlaylist(route) => effects.extend(self.add_context_to_playlist(route).map(Effect::Library)),
			Msg::AddRowToPlaylist(row) => {
				if let Some(i) = self.library.resolve_row(&row) {
					self.add_row_to_playlist(i);
				}
			},
			Msg::AddNowPlayingToPlaylist => self.add_now_playing_to_playlist(),
			Msg::AddToPlaylist(target) => self.add_to_playlist(target, &mut effects),
			Msg::AddToPlaylistDirect { playlist, uris } => effects.push(Effect::Library(self.playlist_add_request(playlist, uris))),
			Msg::RemoveRow(row) => {
				if let Some(i) = self.library.resolve_row(&row) {
					self.remove_row(i, &mut effects);
				}
			},
			Msg::MoveRow(from, to) => self.move_playlist_row(from, to, &mut effects),
			Msg::RenamePlaylist => self.open_playlist_details(),
			Msg::SaveRename => effects.extend(self.save_playlist_details().map(Effect::Library)),
			Msg::CloseDialog => {
				self.dialog = None;
				self.pending_library_dialog = None;
				self.focus = None;
			},
			Msg::QueueContext(route) => effects.push(Effect::Library(library::Request::QueueContext(route))),
			Msg::Discography(i) => self.discography = i,
			Msg::ToggleLike => self.toggle_like(&mut effects),
			Msg::AddToLiked(uri) => self.set_row_liked(uri, true, &mut effects),
			Msg::QueueRemove(i) => self.queue_remove(i, &mut cmds),
			Msg::QueueMoveTo(from, to) => self.queue_move(from, to, &mut cmds),
			Msg::QueueClear => self.queue_clear(&mut cmds),
			Msg::QueueJump(i) => self.queue_jump(i, &mut cmds),
			Msg::QueueRow(row) => {
				if let Some(i) = self.library.resolve_row(&row) {
					self.queue_row(i, false, &mut cmds);
				}
			},
			Msg::QueuePlayNext(row) => {
				if let Some(i) = self.library.resolve_row(&row) {
					self.queue_row(i, true, &mut cmds);
				}
			},
			Msg::PlayListing => self.play_listing(&mut effects),
			Msg::LoadMore => self.load_more(&mut effects),
			Msg::SearchSubmit(text) => self.search_submit(&text, &mut effects),
			Msg::SignIn => effects.push(Effect::SignIn),
			Msg::CancelSignIn => effects.push(Effect::CancelSignIn),
			Msg::SignOut => effects.push(Effect::SignOut),
			Msg::Focus(field) => self.focus = field,
			Msg::SetText(field, text) => self.set_text(field, text, &mut effects),
			Msg::Edit { from, to, text } => self.edit_input(from, to, text, &mut effects),
			Msg::Submit => return self.submit_input(),
			Msg::Pref(pref) => effects.extend(self.set_pref(&pref)),
			Msg::Sleep(choice) => self.set_sleep(choice, Instant::now()),
		}
		effects.splice(0..0, cmds.into_iter().map(Effect::Player));
		for effect in &effects {
			if let Effect::Player(command) = effect {
				self.optimistic_playback(command);
			}
		}
		self.release_hidden_controls();
		effects
	}

	fn close_menus(&mut self, msg: &Msg) {
		// The library filter gives the keys back on a click elsewhere.
		if self.focus == Some(Field::LibraryFilter)
			&& !matches!(msg, Msg::Focus(_) | Msg::SetText(_, _) | Msg::Edit { .. } | Msg::Submit | Msg::ClearLibraryFilter)
		{
			self.focus = None;
		}
		if !matches!(msg, Msg::LibraryMenu | Msg::LibrarySort(_) | Msg::LibraryView(_)) {
			self.library_menu = false;
		}
		if !matches!(msg, Msg::DevicesMenu) {
			self.connect.menu = false;
		}
	}

	fn release_hidden_controls(&mut self) {
		if self.focus.is_some_and(|f| !self.field_visible(f)) {
			self.focus = None;
		}
		if self.library_menu && !self.library_tools() {
			self.library_menu = false;
		}
	}

	fn activate_command(&self, command: Command, cmds: &mut Vec<Command>) {
		if !self.active {
			cmds.push(Command::Activate);
		}
		cmds.push(command);
	}

	fn toggle_play(&mut self, cmds: &mut Vec<Command>) {
		if self.playing() {
			cmds.push(Command::Pause);
		} else {
			if let Some(play) = self.rules_resume() {
				self.activate_command(play, cmds);
			} else if !self.active && !self.item.uri.is_empty() {
				// The bar can hold a passive account snapshot: no local track is loaded yet.
				// An explicit load resumes exactly what is shown, without a transfer that
				// could choose a different track or position from a later account state.
				let position_ms = self.position_ms();
				let start = if self.queue_restored {
					crate::spotify::PlayStart::Restored {
						uri: self.item.uri.clone(),
						context_uri: self.context.uri.clone(),
						position_ms,
						upcoming: self
							.upcoming
							.iter()
							.map(|entry| (entry.uid.clone(), entry.item.uri.clone(), entry.source.clone()))
							.collect(),
					}
				} else if self.context.uri.is_empty() {
					crate::spotify::PlayStart::List { uris: vec![self.item.uri.clone()], position_ms }
				} else {
					crate::spotify::PlayStart::Context {
						uri: self.context.uri.clone(),
						offset_uri: Some(self.item.uri.clone()),
						position_ms,
					}
				};
				cmds.push(Command::Load(start));
			} else {
				self.activate_command(Command::Play { uri: None }, cmds);
			}
		}
	}

	fn seek_absolute(&mut self, position_ms: u64, cmds: &mut Vec<Command>) {
		let duration = self.duration_ms();
		if duration > 0 {
			cmds.push(self.seek(position_ms.min(duration)));
		}
	}

	fn seek_by(&mut self, delta: i64, cmds: &mut Vec<Command>) {
		if self.duration_ms() > 0 {
			let pos = self.position_ms().saturating_add_signed(delta * 1000);
			cmds.push(self.seek(pos.min(self.duration_ms().saturating_sub(1000))));
		}
	}

	fn toggle_mute(&mut self) -> Command {
		let volume = if self.volume > 0 {
			self.unmute_to = self.volume;
			0
		} else if self.unmute_to > 0 {
			self.unmute_to
		} else {
			50
		};
		self.set_volume(volume)
	}

	fn cycle_repeat(&mut self, cmds: &mut Vec<Command>) {
		self.options.repeat = match self.options.repeat {
			Repeat::Off => Repeat::Context,
			Repeat::Context => Repeat::Track,
			Repeat::Track => Repeat::Off,
		};
		cmds.extend(Command::repeat(self.options.repeat));
	}

	fn toggle_panel(&mut self, panel: Panel, cmds: &mut Vec<Command>, effects: &mut Vec<Effect>) {
		if panel == Panel::NowPlaying {
			if self.side_panel() == Some(Panel::NowPlaying) {
				self.now_playing_view = false;
				self.panel = None;
			} else {
				self.now_playing_view = true;
				self.panel = Some(Panel::NowPlaying);
			}
			effects.push(Effect::SaveSettings);
		} else {
			self.panel = if self.panel == Some(panel) { None } else { Some(panel) };
			if self.panel == Some(Panel::Queue) && self.logged_in {
				cmds.push(Command::GetQueue);
			}
		}
	}

	fn toggle_library_kind(&mut self, kind: library::Kind) {
		let kinds = &mut self.library.kinds;
		if let Some(i) = kinds.iter().position(|k| *k == kind) {
			kinds.remove(i);
		} else {
			kinds.push(kind);
		}
	}

	fn clear_library_filter(&mut self) {
		self.library.filter.clear();
		self.focus = Some(Field::LibraryFilter);
	}

	fn toggle_shelf(&mut self, id: &'static str) {
		if !self.open_shelves.remove(id) {
			self.open_shelves.insert(id);
		}
	}

	fn eq_set(&mut self, band: usize, db: f32, effects: &mut Vec<Effect>) {
		if let Some(gain) = self.eq.gains_db.get_mut(band) {
			// Setting its present gain keeps the preset.
			if *gain != db {
				*gain = db;
				self.eq_preset = None;
			}
			self.eq.enabled = true;
			effects.push(Effect::SetEq);
		}
	}

	fn eq_preset(&mut self, name: &'static str, effects: &mut Vec<Effect>) {
		if let Some(preset) = crate::audio::EqSettings::preset(name) {
			self.eq = preset;
			self.eq_preset = Some(name.to_owned());
			effects.push(Effect::SetEq);
		}
	}

	fn navigate(&mut self, route: Route, effects: &mut Vec<Effect>) {
		if route == Route::Search && self.library.route == Route::Search {
			return;
		}
		self.discography = 0;
		effects.extend(self.library.navigate(route).into_iter().map(Effect::Library));
	}

	fn play_row(&mut self, index: usize, effects: &mut Vec<Effect>) {
		self.library.select(index);
		self.played_from = Some(self.library.route.clone());
		effects.extend(self.library.play_start(index).map(|start| Effect::Player(Command::Load(start))));
	}

	fn toggle_follow(&mut self, effects: &mut Vec<Effect>) {
		if let Some(listing) = &mut self.library.listing
			&& let (Some(uri), Some(followed)) = (listing.context_uri.clone(), listing.followed)
		{
			listing.followed = Some(!followed);
			effects.push(Effect::Library(library::Request::SetFollowed(uri, !followed)));
		}
	}

	fn follow_artist(&mut self, id: &str, name: String, effects: &mut Vec<Effect>) {
		let uri = format!("spotify:artist:{id}");
		let followed = self.library.followed.iter().any(|artist| artist.uri == uri);
		if followed {
			self.library.followed.retain(|artist| artist.uri != uri);
		} else {
			let artist = self.library.artists.get(id).cloned().unwrap_or_else(|| crate::spotify::Artist {
				name,
				uri: uri.clone(),
				images: Vec::new(),
				genres: Vec::new(),
			});
			self.library.followed.insert(0, artist);
		}
		if let Some(listing) = &mut self.library.listing
			&& listing.context_uri.as_deref() == Some(uri.as_str())
			&& listing.followed.is_some()
		{
			listing.followed = Some(!followed);
		}
		effects.push(Effect::Library(library::Request::SetFollowed(uri, !followed)));
	}

	fn add_row_to_playlist(&mut self, index: usize) {
		self.pending_library_dialog = None;
		if let Some(track) = self.library.rows().get(index) {
			self.dialog = Some(Dialog::AddToPlaylist { uris: vec![track.uri.clone()], label: track.name.clone() });
		}
	}

	fn add_now_playing_to_playlist(&mut self) {
		self.pending_library_dialog = None;
		if self.item.uri.starts_with("spotify:track:") {
			self.dialog = Some(Dialog::AddToPlaylist { uris: vec![self.item.uri.clone()], label: self.item.name().to_owned() });
		}
	}

	fn add_to_playlist(&mut self, target: Option<String>, effects: &mut Vec<Effect>) {
		if let Some(super::playlist::PendingDialog::AddContext { target: pending, .. }) = &mut self.pending_library_dialog {
			*pending = Some(target);
			return;
		}
		if let Some(Dialog::AddToPlaylist { uris, .. }) = self.dialog.take() {
			let request = match target {
				Some(id) => self.playlist_add_request(id, uris),
				None => library::Request::CreatePlaylist(self.new_playlist_name(), uris),
			};
			effects.push(Effect::Library(request));
		}
	}

	pub(super) fn playlist_add_request(&self, id: String, uris: Vec<String>) -> library::Request {
		let name = self.library.playlists.iter().find(|p| p.id == id).map_or_else(String::new, |p| p.name.clone());
		library::Request::AddToPlaylist(id, name, uris)
	}

	fn remove_row(&self, index: usize, effects: &mut Vec<Effect>) {
		if let (Route::Playlist(id), Some(track), Some(listing)) =
			(&self.library.route, self.library.rows().get(index), &self.library.listing)
			&& listing.editable
			&& let Some(position) = listing.positions.get(index).copied()
		{
			effects.push(Effect::Library(library::Request::RemoveFromPlaylist(id.clone(), track.uri.clone(), position)));
		}
	}

	fn move_playlist_row(&self, from: usize, to: usize, effects: &mut Vec<Effect>) {
		if let (Route::Playlist(id), Some(listing)) = (&self.library.route, &self.library.listing)
			&& listing.editable
			&& from != to
			&& let (Some(source), Some(target), Some(from), Some(to)) =
				(listing.tracks.get(from), listing.tracks.get(to), listing.positions.get(from), listing.positions.get(to))
		{
			effects.push(Effect::Library(library::Request::MovePlaylistRow {
				id:     id.clone(),
				from:   *from,
				to:     *to,
				source: source.uri.clone(),
				target: target.uri.clone(),
			}));
		}
	}

	fn toggle_like(&mut self, effects: &mut Vec<Effect>) {
		if !self.item.uri.is_empty() {
			let liked = self.liked() == Some(true);
			self.library.liked = Some((self.item.uri.clone(), !liked));
			self.library.cache_liked(&self.item.uri, !liked);
			effects.push(Effect::Library(library::Request::SetLiked(self.item.uri.clone(), !liked)));
			self.dialog = None;
		}
	}

	fn set_row_liked(&mut self, uri: String, liked: bool, effects: &mut Vec<Effect>) {
		if !self.library.cache_liked(&uri, liked) {
			return;
		}
		if uri == self.item.uri {
			self.library.liked = Some((uri.clone(), liked));
		}
		effects.push(Effect::Library(library::Request::SetLiked(uri, liked)));
	}

	fn queue_row(&mut self, index: usize, next: bool, cmds: &mut Vec<Command>) {
		if let Some(track) = self.library.rows().get(index).filter(|track| track.uri.starts_with("spotify:track:")) {
			let (uri, name) = (track.uri.clone(), track.name.clone());
			cmds.extend(self.queue_tracks(vec![uri], next));
			self.show_toast(format!("Added “{name}” to the queue"));
		}
	}

	fn play_listing(&mut self, effects: &mut Vec<Effect>) {
		self.played_from = Some(self.library.route.clone());
		let context = self.library.listing.as_ref().and_then(|listing| listing.context_uri.clone());
		match context {
			Some(uri) if matches!(self.library.route, Route::Playlist(_) | Route::Album(_) | Route::Artist(_)) => {
				effects.push(Effect::Player(Command::Load(crate::spotify::PlayStart::Context { uri, offset_uri: None, position_ms: 0 })));
			},
			_ => effects.extend(self.library.play_start(0).map(|start| Effect::Player(Command::Load(start)))),
		}
	}

	fn load_more(&mut self, effects: &mut Vec<Effect>) {
		if let Some(listing) = &mut self.library.listing
			&& let Some(offset) = listing.next_offset
			&& !listing.loading
		{
			listing.loading = true;
			self.library.pending_offset = Some(offset);
			effects.push(Effect::Library(library::Request::Open(self.library.route.clone(), offset)));
		}
	}

	fn search_submit(&mut self, text: &str, effects: &mut Vec<Effect>) {
		let query = text.trim().to_owned();
		self.library.query.clone_from(&query);
		if !query.is_empty() {
			effects.push(Effect::CancelSearchSoon);
			effects.push(Effect::Library(library::Request::Search(query)));
		}
	}
}

fn nonempty_text(mut text: String) -> Option<String> {
	text.truncate(text.trim_end().len());
	let start = text.len() - text.trim_start().len();
	text.drain(..start);
	(!text.is_empty()).then_some(text)
}
