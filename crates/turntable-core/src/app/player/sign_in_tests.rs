//! Offline sign-in cancellation and deadline checks; no browser, account or audio output.

use std::{error::Error, time::Instant};

use super::{AppEvent, Controller, StartError};
use crate::app::{Effect, Msg, State, config::Paths};

#[test]
fn cancel_resets_welcome_and_ignores_old_start_events() -> Result<(), Box<dyn Error>> {
	let root = tempfile::tempdir()?;
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let mut controller = Controller::fixture(&paths, State { starting: true, ..State::default() })?;
	let old = controller.account_generation;
	controller.start_authorization.set_deadline(Some(Instant::now()));
	controller.dispatch(Msg::CancelSignIn);
	assert!(!controller.state.starting_player());
	assert!(!controller.state.starting);
	assert_eq!(controller.state.notice.as_deref(), Some("Sign-in canceled."));
	assert_ne!(controller.account_generation, old);
	assert!(controller.deadline().is_none());
	assert!(controller.retry_start_at.is_none());
	assert!(controller.state.update(Msg::SignIn).iter().any(|effect| matches!(effect, Effect::SignIn)));
	// A fresh attempt is waiting, but late events from the canceled attempt stay stale.
	controller.state.starting = true;
	assert!(!controller.on_event(AppEvent::Progress(old, "stale browser callback".into())));
	assert!(!controller.on_event(AppEvent::EngineReady(old, Err(StartError::Other("stale token exchange".into())))));
	assert!(controller.state.starting);
	assert_eq!(controller.state.notice.as_deref(), Some("Sign-in canceled."));
	assert!(!paths.config.join("credentials.json").exists());
	Ok(())
}

#[test]
fn deadline_cancels_attempt_with_notice_without_automatic_retry() -> Result<(), Box<dyn Error>> {
	let root = tempfile::tempdir()?;
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let mut controller = Controller::fixture(&paths, State { starting: true, ..State::default() })?;
	let old = controller.account_generation;
	let deadline = Instant::now();
	controller.start_authorization.set_deadline(Some(deadline));
	assert_eq!(controller.deadline(), Some(deadline));
	assert!(controller.run_timers());
	assert!(!controller.state.starting);
	assert_ne!(controller.account_generation, old);
	assert_eq!(controller.state.notice.as_deref(), Some("Sign-in timed out. Please try again."));
	assert!(controller.deadline().is_none());
	assert!(controller.retry_start_at.is_none());
	assert!(!controller.on_event(AppEvent::EngineReady(old, Err(StartError::SignInTimedOut))));
	Ok(())
}

#[test]
fn worker_deadline_notice_is_not_treated_as_network_outage() -> Result<(), Box<dyn Error>> {
	let root = tempfile::tempdir()?;
	let paths = Paths { config: root.path().join("config"), cache: root.path().join("cache") };
	let mut controller = Controller::fixture(&paths, State { starting: true, ..State::default() })?;
	assert!(controller.on_event(AppEvent::EngineReady(controller.account_generation, Err(StartError::SignInTimedOut))));
	assert!(!controller.state.starting);
	assert!(!controller.state.offline);
	assert!(controller.retry_start_at.is_none());
	assert_eq!(controller.state.notice.as_deref(), Some("Sign-in timed out. Please try again."));
	Ok(())
}
