//! Local, account-scoped recent search selections; no Spotify requests are made.
//! Songs use their album route and track URI, so songs on the same album dedupe.

use std::{
	fs, io,
	path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::{State, config};
use crate::library::{Route, Tile};

#[derive(Debug, Default)]
pub(crate) struct RecentSearches {
	account: Option<String>,
	tiles:   Vec<Tile>,
}

impl RecentSearches {
	fn path(config: &Path, account: &str) -> PathBuf {
		config.join("recent-searches").join(format!("{:x}.json", Sha256::digest(account.as_bytes())))
	}

	pub(crate) fn load(&mut self, config: &Path, account: Option<String>) {
		if self.account == account {
			return;
		}
		self.tiles.clear();
		if let Some(account) = &account
			&& let Ok(bytes) = fs::read(Self::path(config, account))
			&& let Ok(tiles) = serde_json::from_slice::<Vec<Tile>>(&bytes)
		{
			for tile in tiles {
				if !self.tiles.iter().any(|existing| existing.route == tile.route) {
					self.tiles.push(tile);
					if self.tiles.len() == 10 {
						break;
					}
				}
			}
		}
		self.account = account;
	}

	pub(crate) fn save(&self, config: &Path) -> io::Result<()> {
		let Some(account) = &self.account else { return Ok(()) };
		let path = Self::path(config, account);
		fs::create_dir_all(config.join("recent-searches"))?;
		let bytes = serde_json::to_vec(&self.tiles)?;
		config::write_private(&path, &bytes)
	}

	fn remember(&mut self, tile: Tile) {
		self.forget(&tile.route);
		self.tiles.insert(0, tile);
		self.tiles.truncate(10);
	}

	fn forget(&mut self, route: &Route) {
		self.tiles.retain(|tile| tile.route != *route);
	}

	pub(crate) fn clear(&mut self) {
		self.tiles.clear();
	}
}

impl State {
	/// Recent search selections, newest first, at most ten, unique by route.
	/// A song tile opens its album and plays its track URI; its subtitle is
	/// `Song • Artist`. Two songs on the same album therefore share one entry.
	pub fn recent_searches(&self) -> &[Tile] {
		&self.recent_searches.tiles
	}

	pub(super) fn remember_search(&mut self, tile: Tile) {
		self.recent_searches.remember(tile);
	}

	pub(super) fn forget_search(&mut self, route: &Route) {
		self.recent_searches.forget(route);
	}
}

/// The startup identity comes from librespot's saved credentials, never a Web API lookup.
pub(crate) fn saved_account(config: &Path) -> Option<String> {
	let bytes = fs::read(config.join("credentials.json")).ok()?;
	let credentials: librespot_core::authentication::Credentials = serde_json::from_slice(&bytes).ok()?;
	credentials.username.filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::app::Msg;

	fn tile(id: usize) -> Tile {
		Tile {
			route: Route::Album(id.to_string()),
			play:  Some(format!("spotify:track:{id}")),
			title: id.to_string(),
			sub:   "Song • Artist".into(),
			image: None,
		}
	}

	#[test]
	fn dedupe_moves_to_front_and_replaces_metadata() {
		let mut state = State::default();
		state.update(Msg::RememberSearch(tile(1)));
		state.update(Msg::RememberSearch(tile(2)));
		let mut replacement = tile(1);
		replacement.title = "Updated".into();
		state.update(Msg::RememberSearch(replacement.clone()));
		assert_eq!(state.recent_searches(), &[replacement, tile(2)]);
		state.update(Msg::ForgetSearch(Route::Album("1".into())));
		assert_eq!(state.recent_searches(), &[tile(2)]);
		state.update(Msg::ClearRecentSearches);
		assert!(state.recent_searches().is_empty());
	}

	#[test]
	fn caps_at_ten_newest() {
		let mut state = State::default();
		for id in 0..12 {
			state.update(Msg::RememberSearch(tile(id)));
		}
		assert_eq!(state.recent_searches(), &(2..12).rev().map(tile).collect::<Vec<_>>());
	}

	#[test]
	fn persistence_round_trip_and_account_isolation() {
		let dir = tempfile::tempdir().unwrap();
		let mut searches = RecentSearches::default();
		searches.load(dir.path(), Some("alice".into()));
		searches.remember(tile(1));
		searches.remember(tile(2));
		searches.save(dir.path()).unwrap();
		let mut restored = RecentSearches::default();
		restored.load(dir.path(), Some("alice".into()));
		assert_eq!(restored.tiles, vec![tile(2), tile(1)]);
		restored.load(dir.path(), Some("bob".into()));
		assert!(restored.tiles.is_empty());
		restored.remember(tile(3));
		restored.save(dir.path()).unwrap();
		restored.load(dir.path(), Some("alice".into()));
		assert_eq!(restored.tiles, vec![tile(2), tile(1)]);
		restored.clear();
		restored.save(dir.path()).unwrap();
		restored.load(dir.path(), None);
		assert!(restored.tiles.is_empty());
		restored.load(dir.path(), Some("alice".into()));
		assert!(restored.tiles.is_empty());
		restored.load(dir.path(), Some("bob".into()));
		assert_eq!(restored.tiles, vec![tile(3)]);
	}
}
