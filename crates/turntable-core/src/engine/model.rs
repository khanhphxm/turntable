//! The player's vocabulary: what plays (entities), what changes (events) and what the UI asks
//! for (commands). The engine translates librespot's events and connect state into these.

pub use librespot_connect::QueueEdit;
// ---------------------------------------------------------------- entities

#[derive(Debug, Clone, Default)]
pub struct Entity {
	pub uri:         String,
	pub decorations: Decorations,
}

#[derive(Debug, Clone, Default)]
pub struct Decorations {
	pub identity:        Identity,
	pub visual_identity: VisualIdentity,
	pub parent:          Option<Related>,
	pub creators:        Vec<Related>,
	pub playback:        PlaybackInfo,
}

#[derive(Debug, Clone, Default)]
pub struct Identity {
	pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct VisualIdentity {
	pub cover: Vec<Cover>,
}

#[derive(Debug, Clone, Default)]
pub struct Cover {
	pub url:  String,
	/// `small`, `default`, `large` or `xlarge`.
	pub size: String,
}

#[derive(Debug, Clone, Default)]
pub struct Related {
	pub entity: Box<Entity>,
}

#[derive(Debug, Clone, Default)]
pub struct PlaybackInfo {
	pub duration_ms:     u64,
	pub content_ratings: Vec<String>,
}

impl Entity {
	pub fn name(&self) -> &str {
		&self.decorations.identity.name
	}

	/// Creator names joined with ", " (artists of a track, publisher of an episode).
	pub fn creators(&self) -> String {
		let mut out = String::new();
		for c in &self.decorations.creators {
			let name = c.entity.name();
			if name.is_empty() {
				continue;
			}
			if !out.is_empty() {
				out.push_str(", ");
			}
			out.push_str(name);
		}
		out
	}

	/// The parent's name (album of a track, show of an episode).
	pub fn parent_name(&self) -> &str {
		self.decorations.parent.as_ref().map_or("", |p| p.entity.name())
	}

	pub fn duration_ms(&self) -> u64 {
		self.decorations.playback.duration_ms
	}

	/// The cover URL closest to `want` among the player's size names, falling back
	/// to the parent's covers (tracks often carry the album's art there).
	pub fn cover(&self, want: CoverSize) -> Option<&str> {
		pick_cover(&self.decorations.visual_identity.cover, want)
			.or_else(|| self.decorations.parent.as_ref().and_then(|p| pick_cover(&p.entity.decorations.visual_identity.cover, want)))
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoverSize {
	Small,
	Default,
	Large,
	XLarge,
}

impl CoverSize {
	fn parse(s: &str) -> Option<Self> {
		Some(match s {
			"small" => Self::Small,
			"default" => Self::Default,
			"large" => Self::Large,
			"xlarge" => Self::XLarge,
			_ => return None,
		})
	}
}

fn pick_cover(covers: &[Cover], want: CoverSize) -> Option<&str> {
	covers
		.iter()
		.filter(|c| !c.url.is_empty())
		.min_by_key(|c| {
			let size = CoverSize::parse(&c.size).unwrap_or(CoverSize::Default);
			// Prefer the exact size, then larger (downscaled looks fine), then smaller.
			let rank = size as i32 - want as i32;
			if rank >= 0 { rank * 2 } else { -rank * 2 + 1 }
		})
		.map(|c| c.url.as_str())
}

// ------------------------------------------------------------------ events

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Position {
	pub position_ms:  f64,
	/// The player's wall clock, Unix epoch ms.
	pub timestamp_ms: f64,
	/// 0.0 when not advancing.
	pub speed:        f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
	#[default]
	Idle,
	Playing,
	Paused,
	Buffering,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Repeat {
	#[default]
	Off,
	Context,
	Track,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
	pub shuffle:       bool,
	pub repeat:        Repeat,
	/// Smart Shuffle: Spotify mixes recommended songs into the playlist.
	pub smart_shuffle: bool,
}

impl Options {
	/// Smart Shuffle is on.
	pub fn smart_shuffle(&self) -> bool {
		self.smart_shuffle
	}
}

#[derive(Debug, Clone, Default)]
pub struct QueueEntry {
	pub uid:    String,
	/// `context`, `queue` or `autoplay`.
	pub source: String,
	pub item:   Entity,
}

#[derive(Debug, Clone)]
pub enum Event {
	AuthState { logged_in: bool, is_active: bool, device_name: String },
	TrackChanged { item: Entity },
	PlaybackChanged { status: Status },
	VolumeChanged { volume: u8 },
	DeviceChanged { is_active: bool },
	ContextChanged { context: Entity },
	OptionsChanged { options: Options },
	PositionSync { position: Position },
	QueueChanged { upcoming: Vec<QueueEntry>, revision: u64 },
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
	/// Request the complete native upcoming queue; snapshots also arrive after every change.
	GetQueue,
	Play {
		uri: Option<String>,
	},
	Pause,
	SkipNext,
	SkipPrev,
	Seek {
		position_ms: u64,
	},
	SetVolume {
		volume: u8,
	},
	SetShuffle {
		enabled: bool,
	},
	SetRepeatContext {
		enabled: bool,
	},
	SetRepeatTrack {
		enabled: bool,
	},
	/// Native queue mutation; revision acknowledges optimistic state without restarting playback.
	EditQueue {
		revision: u64,
		edit:     QueueEdit,
	},
	/// Transfers the account's player through the session; `device` empty means this device.
	Transfer {
		device: String,
		play:   bool,
	},
	Activate,
	/// Plays a context or a list of tracks here (activating this device when it isn't).
	Load(crate::spotify::PlayStart),
}

impl Command {
	/// The two commands that set a repeat mode, in the order the API requires.
	pub fn repeat(mode: Repeat) -> [Self; 2] {
		match mode {
			Repeat::Off => [Self::SetRepeatTrack { enabled: false }, Self::SetRepeatContext { enabled: false }],
			Repeat::Context => [Self::SetRepeatTrack { enabled: false }, Self::SetRepeatContext { enabled: true }],
			Repeat::Track => [Self::SetRepeatContext { enabled: false }, Self::SetRepeatTrack { enabled: true }],
		}
	}
}

/// An account's last player state, read without loading audio or activating this device.
#[derive(Debug, Clone)]
pub struct LastPlayback {
	pub item:        Entity,
	pub context:     Entity,
	pub position_ms: u64,
	pub options:     Options,
}

/// A player reported by the account-wide Connect cluster, not this device's decoder.
#[derive(Debug)]
pub struct ClusterPlayback {
	pub playback: LastPlayback,
	pub device:   Option<crate::spotify::Device>,
	pub position: Position,
	pub status:   Status,
}

/// Passive, pushed account state from the authenticated session. Device menus and remote
/// bars use this instead of Web API polling while the session remains connected.
#[derive(Debug)]
pub struct ConnectState {
	pub account:      String,
	pub devices:      Vec<crate::spotify::Device>,
	pub playback:     Option<ClusterPlayback>,
	pub local_active: bool,
	pub initial:      bool,
}

/// What the engine reports to its client.
#[derive(Debug)]
#[expect(
	clippy::large_enum_variant,
	reason = "decoder events and frequent Dealer snapshots share a stream without allocating for every update"
)]
pub enum Incoming {
	Event(Event),
	/// Registration supplied no cluster; permits the recently-played fallback for this account.
	RestoreUnavailable {
		account: String,
	},
	/// A passive account/player/device update from the session's Dealer subscription.
	ConnectState(ConnectState),
	/// A requested native player command failed; the session may remain usable.
	CommandFailed(String),
	/// The player stopped (its session ended); the client is done.
	Closed(String),
}
