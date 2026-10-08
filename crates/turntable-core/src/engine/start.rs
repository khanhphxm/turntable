//! Serializes native credential writes with sign-out, including unfinished browser/Spirc starts.

use std::{
	io,
	net::{SocketAddr, TcpListener, TcpStream},
	path::Path,
	sync::{Arc, Mutex, PoisonError},
	time::Instant,
};

use librespot_core::cache::Cache;

/// Permission for one account's engines to save reusable credentials.
/// Cancel before deleting the account's credentials; late native authentication cannot recreate them.
#[derive(Clone)]
pub struct StartAuthorization(Arc<Mutex<StartState>>);

struct StartState {
	allowed:  bool,
	deadline: Option<Instant>,
	callback: Option<TcpListener>,
}

#[derive(Debug, thiserror::Error)]
#[error("Sign-in timed out. Please try again.")]
pub(crate) struct SignInTimedOut;

impl Default for StartAuthorization {
	fn default() -> Self {
		Self(Arc::new(Mutex::new(StartState { allowed: true, deadline: None, callback: None })))
	}
}

impl StartAuthorization {
	pub fn cancel(&self) {
		let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
		state.allowed = false;
		// The listener is owned here, not by the worker: cancellation closes the port
		// synchronously, so a new attempt can bind it immediately.
		state.callback = None;
	}

	pub(crate) fn set_deadline(&self, deadline: Option<Instant>) {
		self.0.lock().unwrap_or_else(PoisonError::into_inner).deadline = deadline;
	}

	pub(crate) fn deadline(&self) -> Option<Instant> {
		self.0.lock().unwrap_or_else(PoisonError::into_inner).deadline
	}

	pub(super) fn check(&self) -> io::Result<()> {
		Self::check_locked(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
	}

	fn check_locked(state: &mut StartState) -> io::Result<()> {
		if state.deadline.is_some_and(|at| Instant::now() >= at) {
			state.allowed = false;
			state.callback = None;
			return Err(io::Error::new(io::ErrorKind::TimedOut, SignInTimedOut));
		}
		if !state.allowed {
			return Err(io::Error::new(io::ErrorKind::Interrupted, "Sign-in was canceled"));
		}
		Ok(())
	}

	pub(super) fn listen(&self, address: SocketAddr) -> io::Result<SocketAddr> {
		let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
		Self::check_locked(&mut state)?;
		let listener = TcpListener::bind(address)?;
		listener.set_nonblocking(true)?;
		let address = listener.local_addr()?;
		state.callback = Some(listener);
		Ok(address)
	}

	pub(super) fn accept_callback(&self) -> io::Result<TcpStream> {
		let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
		Self::check_locked(&mut state)?;
		state.callback.as_ref().ok_or_else(|| io::Error::other("Sign-in listener is closed"))?.accept().map(|(stream, _)| stream)
	}

	pub(super) fn stop_listening(&self) {
		self.0.lock().unwrap_or_else(PoisonError::into_inner).callback = None;
	}

	pub(super) fn guard_cache(&self, cache: &mut Cache, directory: &Path) {
		let authorization = self.clone();
		let path = directory.join(super::CREDENTIALS);
		cache.set_credentials_writer(move |credentials| {
			let mut allowed = authorization.0.lock().unwrap_or_else(PoisonError::into_inner);
			if Self::check_locked(&mut allowed).is_err() {
				return Ok(());
			}
			// Retain the lock through the atomic write. Cancellation waits for any earlier
			// write, then sign-out deletes it; no later write can pass the permission check.
			let bytes = serde_json::to_vec(credentials).map_err(io::Error::other)?;
			crate::app::config::write_private(&path, &bytes)
		});
	}
}

#[cfg(test)]
mod tests {
	use std::{io, sync::mpsc, thread, time::Instant};

	use librespot_core::{authentication::Credentials, cache::Cache};

	use super::{SignInTimedOut, StartAuthorization};

	#[test]
	fn late_account_start_cannot_persist_credentials_after_signout_or_replace_new_account() -> Result<(), Box<dyn std::error::Error>> {
		let directory = tempfile::tempdir()?;
		let mut cache = Cache::new(Some(directory.path()), None, None, None)?;
		let old = StartAuthorization::default();
		old.guard_cache(&mut cache, directory.path());
		let old_cache = cache.clone();
		let (authenticated, late) = mpsc::channel();
		let start = thread::spawn(move || -> Result<(), mpsc::RecvError> {
			// Represents native authentication completing after the controller signed out.
			late.recv()?;
			old_cache.save_credentials(&Credentials { username: Some("old-account".into()), ..Default::default() });
			Ok(())
		});
		old.cancel();
		super::super::forget_sign_in(directory.path())?;
		assert!(old.check().is_err());
		authenticated.send(())?;
		start.join().map_err(|_panic| io::Error::other("late account worker panicked"))??;
		assert!(!super::super::has_sign_in(directory.path()));
		let new = StartAuthorization::default();
		new.guard_cache(&mut cache, directory.path());
		cache.save_credentials(&Credentials { username: Some("new-account".into()), ..Default::default() });
		let mut stale = cache.clone();
		old.guard_cache(&mut stale, directory.path());
		stale.save_credentials(&Credentials { username: Some("old-account".into()), ..Default::default() });
		assert_eq!(cache.credentials().ok_or("new account credentials missing")?.username.as_deref(), Some("new-account"));
		Ok(())
	}

	#[test]
	fn canceled_or_expired_start_cannot_save_without_controller_pump() -> Result<(), Box<dyn std::error::Error>> {
		for expired in [false, true] {
			let directory = tempfile::tempdir()?;
			let mut cache = Cache::new(Some(directory.path()), None, None, None)?;
			let authorization = StartAuthorization::default();
			authorization.guard_cache(&mut cache, directory.path());
			if expired {
				authorization.set_deadline(Some(Instant::now()));
			} else {
				authorization.cancel();
			}
			cache.save_credentials(&Credentials { username: Some("late-account".into()), ..Default::default() });
			assert!(!super::super::has_sign_in(directory.path()));
			let error = authorization.check().err().ok_or("revoked start was accepted")?;
			assert_eq!(error.get_ref().is_some_and(|source| source.is::<SignInTimedOut>()), expired);
			// Clearing a timer cannot restore a revoked attempt's credential permission.
			authorization.set_deadline(None);
			assert!(authorization.check().is_err());
		}
		Ok(())
	}
}
