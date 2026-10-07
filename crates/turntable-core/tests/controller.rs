//! Host integration without a terminal or the user's Spotify credentials.

#![expect(
	unused_crate_dependencies,
	reason = "the host integration tests exercise the public library API, not its implementation dependencies"
)]

#[cfg(test)]
mod tests {
	use std::{
		fs,
		sync::{Arc, mpsc},
		time::Duration,
	};

	use turntable_core::{Controller, Msg, Options, Paths, library::Conn};

	#[test]
	fn workers_wake_the_host_and_pump_needs_no_terminal() {
		let root = std::env::temp_dir().join(format!("turntable-controller-{}", std::process::id()));
		let _ = fs::remove_dir_all(&root);
		let paths = Paths { config: root.join("config"), cache: root.join("cache") };
		let (tx, rx) = mpsc::channel();
		let wake = Arc::new(move || {
			let _ = tx.send(());
		});
		let mut controller = Controller::new(&paths, Options { device_name: Some("Test host".into()) }, wake).unwrap();
		assert_eq!(controller.state().device_name, "Test host");
		assert!(!controller.state().signed_in);
		rx.recv_timeout(Duration::from_secs(5)).unwrap();
		assert!(controller.pump(), "the library worker's setup response invalidates the host");
		assert_eq!(controller.state().library.conn, Conn::NeedsClientId);
		assert!(!controller.pump(), "an idle controller does not require another redraw");
		assert!(controller.deadline().is_none(), "no player or panel timer is active");
		assert!(controller.dispatch(Msg::Quit));
		assert!(controller.state().quit);
		drop(controller);
		fs::remove_dir_all(root).unwrap();
	}

	#[test]
	fn construction_reports_directory_errors_before_starting_workers() {
		let root = std::env::temp_dir().join(format!("turntable-controller-file-{}", std::process::id()));
		fs::write(&root, b"not a directory").unwrap();
		let paths = Paths { config: root.join("config"), cache: root.join("cache") };
		assert!(Controller::new(&paths, Options::default(), Arc::new(|| {})).is_err());
		fs::remove_file(root).unwrap();
	}
}
