//! The authenticated session owns identity and subscription; no `/me` Web API read.
//! ProductInfo attributes supply the plan. Display names use the same session's
//! spclient profile only if absent from attributes and the account-scoped snapshot.

use super::Reader;
use crate::{
	net::trace,
	spotify::{self, User},
};

#[derive(serde::Deserialize)]
struct Profile {
	name: Option<String>,
}

impl Reader {
	pub(in crate::library) fn identity(&self) -> (User, Option<String>) {
		let session = self.shared.session();
		let product = session.get_user_attribute("product");
		let kind = session.get_user_attribute("type");
		let plan = product.or(kind);
		let display_name = session
			.get_user_attribute("display_name")
			.or_else(|| session.get_user_attribute("display-name"))
			.filter(|name| !name.is_empty());
		(User { id: session.username(), display_name }, plan)
	}

	pub(in crate::library) fn profile(&self, cached_name: Option<&str>) -> spotify::Result<(User, Option<String>)> {
		let (mut user, plan) = self.identity();
		if user.display_name.is_none() {
			user.display_name = cached_name.map(str::to_owned);
		}
		if user.display_name.is_none() && !matches!(plan.as_deref(), Some("free" | "open")) {
			let bytes = self
				.shared
				.block_on(async {
					let span = trace::start();
					let bytes = self.shared.session().spclient().get_user_profile(&user.id, Some(0), Some(0)).await?;
					span.done("session", "GET", "https://spclient.spotify.com/user-profile-view/v3/profile/me", trace::Reply {
						status: 200,
						bytes: bytes.len(),
						..trace::Reply::default()
					});
					Ok::<_, librespot_core::Error>(bytes)
				})
				.map_err(spotify::Error::from)?;
			let profile: Profile = serde_json::from_slice(&bytes)?;
			user.display_name = profile.name.filter(|name| !name.is_empty());
		}
		Ok((user, plan))
	}

	#[cfg(test)]
	pub(in crate::library) fn for_test(shared: crate::engine::SessionReader) -> Self {
		let mut reader = Self::new(shared);
		reader.roots = Some((std::time::Instant::now(), super::rootlist::Data::default()));
		reader.collection = Some((std::time::Instant::now(), Vec::new()));
		reader
	}
}

#[cfg(test)]
mod tests {
	use librespot_core::{Session, SessionConfig};

	use super::*;

	#[test]
	fn profile_uses_session_identity_name_and_authoritative_product() -> Result<(), Box<dyn std::error::Error>> {
		let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build()?;
		let session = {
			let _context = runtime.enter();
			Session::new(SessionConfig::default(), None)
		};
		session.set_username("alice");
		let _ = session.set_user_attribute("display_name", "Alice");
		let _ = session.set_user_attribute("product", "premium");
		let (_stop, rx) = tokio::sync::watch::channel(false);
		let reader = Reader::new(crate::engine::SessionReader::from_session(session.clone(), runtime.handle().clone(), rx));
		let (user, plan) = reader.profile(None)?;
		assert_eq!(user.id, "alice");
		assert_eq!(user.display_name.as_deref(), Some("Alice"));
		assert_eq!(plan.as_deref(), Some("premium"));
		let _ = session.set_user_attribute("type", "premium");
		let _ = session.set_user_attribute("product", "free");
		assert_eq!(reader.identity().1.as_deref(), Some("free"));
		Ok(())
	}
}
