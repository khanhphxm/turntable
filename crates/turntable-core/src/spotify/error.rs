use std::{fmt, io, time::Duration};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
	/// Transport failure: DNS, connect, TLS, timeout.
	Network(String),
	/// Non-success status other than 401/429. `message` is a short summary of the body.
	Http {
		status:  u16,
		message: String,
	},
	/// 429 without a quota reason: back off for `retry_after` and try again.
	RateLimited {
		retry_after: Option<Duration>,
	},
	/// 429 with `reason: "QUOTA_EXCEEDED"`: the Development Mode budget is spent.
	/// Retrying soon will not help.
	QuotaExceeded {
		retry_after: Option<Duration>,
	},
	/// No caller-supplied Web API token. This never invalidates native authentication.
	AuthRequired,
	/// The caller-supplied Web API token was rejected and cannot be refreshed.
	AuthExpired,
	/// The native playback session was rejected; Web API token errors never invalidate it.
	NativeAuthRequired,
	/// 404 from the player endpoints: Spotify does not know the target device, or no device is active.
	DeviceNotFound,
	/// 403 `PREMIUM_REQUIRED`: Spotify only lets Premium accounts control playback.
	PremiumRequired,
	/// A response did not have the expected shape.
	Parse(String),
	Io(io::Error),
}

impl Error {
	/// A 403: the token lacks a scope, or Development Mode doesn't serve this item.
	pub fn is_forbidden(&self) -> bool {
		matches!(self, Self::Http { status: 403, .. })
	}

	/// One calm sentence for the UI (Display is for logs and the CLI).
	pub fn user_message(&self) -> String {
		match self {
			Self::Network(_) => "Can't reach Spotify. Check your connection.".into(),
			Self::Http { status, .. } if *status >= 500 => "Spotify is having trouble right now. Try again in a moment.".into(),
			Self::Http { status: 403, .. } => "Spotify doesn't allow that for this item.".into(),
			Self::Http { status: 404, .. } => "That isn't available on Spotify anymore.".into(),
			Self::Http { .. } | Self::Parse(_) | Self::Io(_) => "Something went wrong talking to Spotify.".into(),
			Self::RateLimited { .. } => "Spotify asked us to slow down. Try again in a few seconds.".into(),
			Self::QuotaExceeded { retry_after } => match retry_after {
				Some(d) if d.as_secs() >= 3600 => {
					format!("Your Spotify app hit its daily request limit. It resets in about {} h.", d.as_secs().div_ceil(3600))
				},
				Some(d) => format!("Your Spotify app hit its request limit. It resets in about {} min.", d.as_secs().div_ceil(60).max(1)),
				None => "Your Spotify app hit its request limit. It resets later today.".into(),
			},
			Self::AuthRequired | Self::AuthExpired | Self::NativeAuthRequired => "Reconnect your Spotify account.".into(),
			Self::DeviceNotFound => "Spotify doesn't see the player right now.".into(),
			Self::PremiumRequired => "turntable needs Spotify Premium, and this account isn't on it.".into(),
		}
	}
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Network(msg) => write!(f, "network error: {msg}"),
			Self::Http { status, message } if message.is_empty() => write!(f, "spotify returned HTTP {status}"),
			Self::Http { status, message } => write!(f, "spotify returned HTTP {status}: {message}"),
			Self::RateLimited { retry_after: Some(d) } => {
				write!(f, "rate limited by spotify, retry in {}s", d.as_secs())
			},
			Self::RateLimited { retry_after: None } => f.write_str("rate limited by spotify"),
			Self::QuotaExceeded { retry_after: Some(d) } => {
				write!(f, "spotify API quota exceeded, retry in {}s", d.as_secs())
			},
			Self::QuotaExceeded { retry_after: None } => f.write_str("spotify API quota exceeded"),
			Self::AuthRequired => f.write_str("no Spotify Web API token supplied"),
			Self::AuthExpired => f.write_str("Spotify Web API token expired"),
			Self::NativeAuthRequired => f.write_str("native Spotify session expired"),
			Self::DeviceNotFound => f.write_str("spotify does not see the player device (not connected yet?)"),
			Self::PremiumRequired => f.write_str("spotify premium is required to control playback"),
			Self::Parse(msg) => write!(f, "unexpected spotify response: {msg}"),
			Self::Io(err) => err.fmt(f),
		}
	}
}

impl std::error::Error for Error {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		match self {
			Self::Io(err) => Some(err),
			_ => None,
		}
	}
}

impl Clone for Error {
	/// Callers that waited for one request share its answer, errors included. An I/O error can't be
	/// cloned, so its copy keeps the kind and the text.
	fn clone(&self) -> Self {
		match self {
			Self::Network(msg) => Self::Network(msg.clone()),
			Self::Http { status, message } => Self::Http { status: *status, message: message.clone() },
			Self::RateLimited { retry_after } => Self::RateLimited { retry_after: *retry_after },
			Self::QuotaExceeded { retry_after } => Self::QuotaExceeded { retry_after: *retry_after },
			Self::AuthRequired => Self::AuthRequired,
			Self::AuthExpired => Self::AuthExpired,
			Self::NativeAuthRequired => Self::NativeAuthRequired,
			Self::DeviceNotFound => Self::DeviceNotFound,
			Self::PremiumRequired => Self::PremiumRequired,
			Self::Parse(msg) => Self::Parse(msg.clone()),
			Self::Io(err) => Self::Io(io::Error::new(err.kind(), err.to_string())),
		}
	}
}

impl From<io::Error> for Error {
	fn from(err: io::Error) -> Self {
		Self::Io(err)
	}
}

impl From<librespot_core::Error> for Error {
	fn from(err: librespot_core::Error) -> Self {
		if err.kind == librespot_core::error::ErrorKind::Unauthenticated {
			Self::NativeAuthRequired
		} else {
			Self::Network(err.to_string())
		}
	}
}

impl From<serde_json::Error> for Error {
	fn from(err: serde_json::Error) -> Self {
		Self::Parse(err.to_string())
	}
}
