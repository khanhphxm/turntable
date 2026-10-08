//! Subscription gate for browser sign-in and every saved-credential startup.
//! ProductInfo completion, not elapsed time or the catalogue `type`, resolves unknown plans.

use std::{fmt, io, time::Duration};

use librespot_core::{AuthenticationError, session::SessionError};
use librespot_protocol::keyexchange::ErrorCode;

use super::Session;

/// A sign-in refused because Spotify does not permit this account's third-party playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInError {
	PremiumRequired { account: Option<String>, product: Option<String> },
}

impl SignInError {
	/// Welcome-page explanation, including the session's display name or account ID when known.
	pub fn message(&self) -> String {
		let Self::PremiumRequired { account, product } = self;
		let account = account.as_deref().unwrap_or("This account");
		let reason = match product.as_deref().map(str::trim) {
			Some("free" | "open") => format!("{account} is on Spotify Free, which doesn't allow playback in third-party apps."),
			_ => format!("Spotify requires a Premium subscription for {account} to play in third-party apps."),
		};
		format!("turntable needs Spotify Premium. {reason} Upgrade at spotify.com/premium, or sign in with a Premium account.")
	}

	/// Dispatch `Msg::OpenUrl` with this URL to offer an Upgrade action.
	pub const fn upgrade_url(&self) -> &'static str {
		"https://www.spotify.com/premium/"
	}
}

impl fmt::Display for SignInError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.message())
	}
}

impl std::error::Error for SignInError {}

/// Only a definite Free/Open answer denies eligibility. ProductInfo may identify
/// the subscription as `type`; `product` can be an XML container's whitespace.
/// Absence and unrecognised values are not evidence that an account is Free.
fn decision(product: Option<&str>, kind: Option<&str>, product_info_received: bool) -> Option<bool> {
	let product = product.map(str::trim);
	let kind = kind.map(str::trim);
	if matches!(product, Some("free" | "open")) || matches!(kind, Some("free" | "open")) {
		Some(false)
	} else if product == Some("premium") || kind == Some("premium") || product_info_received {
		Some(true)
	} else {
		None
	}
}

fn rejection(session: &Session, fallback_account: Option<&str>) -> SignInError {
	let account = session
		.get_user_attribute("display_name")
		.or_else(|| session.get_user_attribute("display-name"))
		.filter(|name| !name.is_empty())
		.or_else(|| {
			let username = session.username();
			(!username.is_empty() && username != "UNKNOWN").then_some(username)
		})
		.or_else(|| fallback_account.filter(|name| !name.is_empty()).map(str::to_owned));
	let product = session
		.get_user_attribute("product")
		.filter(|product| matches!(product.trim(), "free" | "open"))
		.or_else(|| session.get_user_attribute("type"));
	SignInError::PremiumRequired { account, product }
}

/// A pending ProductInfo can arrive slowly. Lack of an answer, including a
/// disconnected session or timeout, must never delete a saved sign-in.
pub(super) async fn wait(session: &Session, fallback_account: Option<&str>) -> io::Result<()> {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
	loop {
		// Acquire completion before reading the attributes it publishes.
		let complete = session.has_product_info();
		let product = session.get_user_attribute("product");
		let kind = session.get_user_attribute("type");
		match decision(product.as_deref(), kind.as_deref(), complete) {
			Some(accepted) => {
				if crate::net::trace::enabled() {
					crate::net::trace::mark(
						"subscription",
						&format!("type={kind:?} product={product:?} has_product_info={complete} accepted={accepted}"),
					);
				}
				return if accepted { Ok(()) } else { Err(io::Error::other(rejection(session, fallback_account))) };
			},
			None if session.is_invalid() || tokio::time::Instant::now() >= deadline => {
				if crate::net::trace::enabled() {
					crate::net::trace::mark(
						"subscription",
						&format!("type={kind:?} product={product:?} has_product_info={complete} accepted=true unconfirmed=true"),
					);
				}
				return Ok(());
			},
			None => tokio::time::sleep(Duration::from_millis(25)).await,
		}
	}
}

pub(super) fn connection_error(error: librespot_core::Error, session: &Session, fallback_account: Option<&str>) -> io::Error {
	let auth = error.error.downcast_ref::<AuthenticationError>().or_else(|| match error.error.downcast_ref::<SessionError>() {
		Some(SessionError::AuthenticationError(auth)) => Some(auth),
		_ => None,
	});
	let premium_required = matches!(auth, Some(AuthenticationError::LoginFailed(ErrorCode::PremiumAccountRequired)));
	let complete = session.has_product_info();
	if premium_required
		|| decision(session.get_user_attribute("product").as_deref(), session.get_user_attribute("type").as_deref(), complete)
			== Some(false)
	{
		io::Error::other(rejection(session, fallback_account))
	} else {
		io::Error::other(error)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn premium_including_family_duo_and_student_product_is_accepted() {
		// Spotify reports product=premium for all paid plan variants, regardless of catalogue type.
		for plan in ["premium", "family", "duo", "student"] {
			let session = Session::new(librespot_core::SessionConfig::default(), None);
			let _ = session.set_user_attribute("type", plan);
			let _ = session.set_user_attribute("product", "premium");
			wait(&session, None).await.unwrap();
		}
	}

	#[tokio::test]
	async fn premium_catalogue_with_missing_or_container_product_is_accepted() {
		for product in [None, Some("\n  "), Some("unknown")] {
			let session = Session::new(librespot_core::SessionConfig::default(), None);
			let _ = session.set_user_attribute("type", "premium");
			if let Some(product) = product {
				let _ = session.set_user_attribute("product", product);
			}
			wait(&session, None).await.unwrap();
		}
	}

	#[tokio::test]
	async fn disconnected_unconfirmed_session_is_not_rejected() {
		let session = Session::new(librespot_core::SessionConfig::default(), None);
		session.shutdown();
		wait(&session, None).await.unwrap();
	}

	#[test]
	fn only_definite_free_and_open_products_or_types_are_rejected() {
		for free in ["free", "open"] {
			assert_eq!(decision(Some(free), None, true), Some(false));
			assert_eq!(decision(None, Some(free), true), Some(false));
		}
		for product in [Some("unknown"), Some("family"), Some(""), Some("\n  "), None] {
			assert_eq!(decision(product, None, true), Some(true));
			assert_eq!(decision(product, Some("premium"), true), Some(true));
		}
	}

	#[test]
	fn unknown_during_connection_waits() {
		assert_eq!(decision(None, None, false), None);
	}

	#[test]
	fn premium_connection_error_preserves_account_and_reason() {
		let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
		let _context = runtime.enter();
		let session = Session::new(librespot_core::SessionConfig::default(), None);
		let error = connection_error(AuthenticationError::LoginFailed(ErrorCode::PremiumAccountRequired).into(), &session, Some("alice"));
		let error = error.get_ref().unwrap().downcast_ref::<SignInError>().unwrap();
		assert!(error.message().contains("alice"));
		assert!(error.message().contains("Spotify Premium"));
		let network = connection_error(librespot_core::Error::unavailable("offline"), &session, None);
		assert!(network.get_ref().unwrap().downcast_ref::<SignInError>().is_none());
	}

	#[tokio::test]
	async fn slow_product_is_not_rejected_and_first_definite_answer_wins() {
		let session = Session::new(librespot_core::SessionConfig::default(), None);
		let _ = session.set_user_attribute("type", "unknown");
		assert!(tokio::time::timeout(Duration::from_millis(60), wait(&session, None)).await.is_err());
		session.set_username("alice");
		let _ = session.set_user_attribute("display_name", "Alice");
		let _ = session.set_user_attribute("product", "free");
		let error = wait(&session, None).await.unwrap_err();
		let error = error.get_ref().unwrap().downcast_ref::<SignInError>().unwrap();
		assert!(error.message().contains("Alice is on Spotify Free"));
		let _ = session.set_user_attribute("product", "premium");
		wait(&session, None).await.unwrap();
	}
}
