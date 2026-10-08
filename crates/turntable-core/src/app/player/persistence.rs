//! Serialized preference writes. The controller debounces edits before cloning state;
//! this worker owns encoding, durability flushes and ordered recent-search saves.

use std::{
	path::{Path, PathBuf},
	sync::mpsc,
	thread::{self, JoinHandle},
};

use crate::app::{recent_searches::RecentSearches, settings::Settings};

pub(super) enum Save {
	Settings(Settings),
	Searches(RecentSearches),
}

pub(super) struct Saver {
	tx:     Option<mpsc::Sender<Save>>,
	thread: Option<JoinHandle<()>>,
}

impl Saver {
	pub(super) fn spawn(config: &Path, failed: impl Fn(String) + Send + 'static) -> std::io::Result<Self> {
		let config: PathBuf = config.to_owned();
		let (tx, rx) = mpsc::channel();
		let thread = thread::Builder::new().name("settings-save".into()).spawn(move || {
			while let Ok(save) = rx.recv() {
				let (label, result) = match save {
					Save::Settings(settings) => ("settings", settings.save(&config)),
					Save::Searches(searches) => ("recent searches", searches.save(&config)),
				};
				if let Err(error) = result {
					failed(format!("Couldn't save {label}: {error}"));
				}
			}
		})?;
		Ok(Self { tx: Some(tx), thread: Some(thread) })
	}

	pub(super) fn send(&self, save: Save) {
		if let Some(tx) = &self.tx {
			let _ = tx.send(save);
		}
	}
}

impl Drop for Saver {
	fn drop(&mut self) {
		self.tx.take();
		// Shutdown is the only barrier: queued final preferences must survive process exit.
		if let Some(thread) = self.thread.take() {
			let _ = thread.join();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn queued_preferences_are_written_in_order_and_flushed_on_shutdown() {
		let directory = tempfile::tempdir().unwrap();
		let saver = Saver::spawn(directory.path(), |error| panic!("{error}")).unwrap();
		let mut settings = Settings::default();
		settings.eq.enabled = true;
		saver.send(Save::Settings(settings.clone()));
		settings.eq.gains_db[0] = 6.5;
		saver.send(Save::Settings(settings.clone()));
		drop(saver);
		assert_eq!(Settings::load(directory.path()), settings);
	}

	#[test]
	fn equalizer_edits_debounce_without_writing_on_dispatch_and_flush_the_latest_value() {
		let directory = tempfile::tempdir().unwrap();
		let paths = crate::Paths { config: directory.path().join("config"), cache: directory.path().join("cache") };
		let mut controller = crate::Controller::fixture(&paths, crate::State::default()).unwrap();
		for step in 0..=48 {
			controller.dispatch(crate::Msg::EqBand(0, step));
		}
		assert!(!paths.config.join("settings.json").exists());
		assert!(controller.save_at.is_some_and(|at| at > std::time::Instant::now()));
		controller.save_at = Some(std::time::Instant::now());
		controller.pump();
		assert!(!controller.settings_dirty);
		drop(controller);
		assert_eq!(Settings::load(&paths.config).eq.gains_db[0], 12.0);
	}
}
