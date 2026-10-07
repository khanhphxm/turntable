//! Demo data for designing the views before they're bound to `turntable_core`'s state.

pub struct Artist {
	pub name:      &'static str,
	pub color:     u32,
	pub listeners: u32,
	pub about:     &'static str,
}

pub struct Album {
	pub name:   &'static str,
	pub artist: usize,
	pub year:   u16,
	pub kind:   &'static str,
	pub color:  u32,
	pub tracks: Vec<usize>,
}

pub struct Playlist {
	pub name:        &'static str,
	pub owner:       &'static str,
	pub description: &'static str,
	pub color:       u32,
	pub tracks:      Vec<usize>,
}

pub struct Track {
	pub title:  &'static str,
	pub artist: usize,
	pub album:  usize,
	pub ms:     u64,
	/// In Liked Songs.
	pub liked:  bool,
	pub plays:  u64,
}

/// A list of tracks that can be opened and played from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Collection {
	Liked,
	Playlist(usize),
	Album(usize),
	/// An artist's popular tracks.
	Artist(usize),
}

pub struct Library {
	pub artists:   Vec<Artist>,
	pub albums:    Vec<Album>,
	pub playlists: Vec<Playlist>,
	pub tracks:    Vec<Track>,
}

const ARTISTS: [(&str, u32, u32, &str); 8] = [
	("ADOY", 0x5fb3a8, 1_204_331, "Seoul synth-pop band formed in 2017, known for hazy, sunlit guitar pop."),
	("Phum Viphurit", 0xd9a441, 2_877_412, "Thai singer-songwriter raised in New Zealand, writing breezy, wry pop songs."),
	("HYUKOH", 0xc4553a, 3_102_118, "Korean indie rock band whose records drift between rock, funk and lo-fi."),
	("Men I Trust", 0x8aa86b, 5_874_009, "Montreal band of soft grooves and dreamy vocals, self-releasing since 2014."),
	("Yerin Baek", 0x6d7fc4, 2_331_870, "Korean singer and producer, bilingual ballads and late-night R&B."),
	("Khruangbin", 0xb45f8c, 9_210_455, "Houston trio playing global funk, surf and dub-soaked psychedelia."),
	("Mac DeMarco", 0x4f8fd1, 8_440_217, "Canadian songwriter of slacker rock and gentle, warbling ballads."),
	("Tom Misch", 0xe07b39, 7_662_904, "London guitarist and producer blending jazz, soul and house."),
];

const ALBUMS: [(&str, usize, u16, &str, u32); 10] = [
	("CATNIP", 0, 2017, "EP", 0x5fb3a8),
	("Manchild", 1, 2019, "Single", 0xd9a441),
	("23", 2, 2017, "Album", 0xc4553a),
	("Oncle Jazz", 3, 2019, "Album", 0x8aa86b),
	("Every letter I sent you.", 4, 2019, "Album", 0x6d7fc4),
	("Con Todo El Mundo", 5, 2018, "Album", 0xb45f8c),
	("Salad Days", 6, 2014, "Album", 0x4f8fd1),
	("Geography", 7, 2018, "Album", 0xe07b39),
	("Untitled Love", 3, 2022, "Album", 0x9c8f6a),
	("Pancake", 0, 2019, "Album", 0x3f7d92),
];

const TITLES: [&str; 24] = [
	"Wonder",
	"Lover Boy",
	"Comes and Goes",
	"Show Me How",
	"Square",
	"Maria También",
	"Chamber of Reflection",
	"It Runs Through Me",
	"Grapefruit",
	"Tadow",
	"Lazy Love",
	"Sunny Day",
	"Tomboy",
	"Hours",
	"Tawny",
	"Lucky Twice",
	"Movie",
	"Disco Man",
	"Freaking Out the Neighborhood",
	"Water Baby",
	"Glider",
	"August 10",
	"Bonfire",
	"Jung Ji So",
];

const PLAYLISTS: [(&str, &str, u32); 9] = [
	("nordic nights", "cold air, warm guitars", 0x3a6f8f),
	("late night drive", "windows down, city lights", 0x6a3a8f),
	("sunday morning", "slow coffee", 0xc49a3a),
	("focus", "", 0x3a8f5c),
	("kpop indie", "the other side of seoul", 0x8f3a5c),
	("city pop", "", 0xd16f9a),
	("rainy days", "", 0x4a5a6f),
	("gym", "", 0xb33a3a),
	("throwbacks", "", 0x8f7a3a),
];

impl Library {
	pub fn demo() -> Self {
		let artists = ARTISTS.iter().map(|&(name, color, listeners, about)| Artist { name, color, listeners, about }).collect();
		let mut albums: Vec<Album> =
			ALBUMS.iter().map(|&(name, artist, year, kind, color)| Album { name, artist, year, kind, color, tracks: Vec::new() }).collect();
		let mut tracks = Vec::new();
		for (a, album) in albums.iter_mut().enumerate() {
			let count = match album.kind {
				"Single" => 1,
				"EP" => 5,
				_ => 10,
			};
			for k in 0..count {
				let i = tracks.len();
				album.tracks.push(i);
				tracks.push(Track {
					title:  TITLES[(a * 7 + k * 5) % TITLES.len()],
					artist: album.artist,
					album:  a,
					ms:     150_000 + ((i * 37_123 + a * 911) % 140_000) as u64,
					liked:  (i * 7 + a) % 4 == 0,
					plays:  5_000_000 + ((i as u64 * 7_919_113) % 240_000_000),
				});
			}
		}
		let n = tracks.len();
		let playlists = PLAYLISTS
			.iter()
			.enumerate()
			.map(|(p, &(name, description, color))| Playlist {
				name,
				owner: "khanh",
				description,
				color,
				tracks: (0..24 + p * 6).map(|i| (i * 13 + p * 5) % n).collect(),
			})
			.collect();
		Self { artists, albums, playlists, tracks }
	}

	pub fn liked(&self) -> Vec<usize> {
		self.tracks.iter().enumerate().filter(|(_, t)| t.liked).map(|(i, _)| i).collect()
	}

	/// The tracks a collection plays, in order.
	pub fn tracks_of(&self, c: Collection) -> Vec<usize> {
		match c {
			Collection::Liked => self.liked(),
			Collection::Playlist(p) => self.playlists.get(p).map(|p| p.tracks.clone()).unwrap_or_default(),
			Collection::Album(a) => self.albums.get(a).map(|a| a.tracks.clone()).unwrap_or_default(),
			Collection::Artist(a) => {
				let mut t: Vec<usize> = self.tracks.iter().enumerate().filter(|(_, t)| t.artist == a).map(|(i, _)| i).collect();
				t.sort_by_key(|&i| std::cmp::Reverse(self.tracks.get(i).map_or(0, |t| t.plays)));
				t.truncate(10);
				t
			},
		}
	}

	/// Spotify's ✓: in Liked Songs or any of the user's playlists.
	pub fn added(&self, track: usize) -> bool {
		self.tracks.get(track).is_some_and(|t| t.liked) || self.playlists.iter().any(|p| p.tracks.contains(&track))
	}

	pub fn color_of(&self, c: Collection) -> u32 {
		match c {
			Collection::Liked => 0x5038a0,
			Collection::Playlist(p) => self.playlists.get(p).map_or(0x333333, |p| p.color),
			Collection::Album(a) => self.albums.get(a).map_or(0x333333, |a| a.color),
			Collection::Artist(a) => self.artists.get(a).map_or(0x333333, |a| a.color),
		}
	}

	pub fn name_of(&self, c: Collection) -> &str {
		match c {
			Collection::Liked => "Liked Songs",
			Collection::Playlist(p) => self.playlists.get(p).map_or("", |p| p.name),
			Collection::Album(a) => self.albums.get(a).map_or("", |a| a.name),
			Collection::Artist(a) => self.artists.get(a).map_or("", |a| a.name),
		}
	}

	pub fn track_color(&self, track: usize) -> u32 {
		self.tracks.get(track).and_then(|t| self.albums.get(t.album)).map_or(0x333333, |a| a.color)
	}

	pub fn artist_name(&self, artist: usize) -> &str {
		self.artists.get(artist).map_or("", |a| a.name)
	}
}

pub const LYRICS: [&str; 16] = [
	"I wonder if you feel the same",
	"When the night comes falling down",
	"Every street light knows my name",
	"In this quiet part of town",
	"♪",
	"And I keep the radio low",
	"So the city can't hear",
	"All the things I never told",
	"While you were here",
	"♪",
	"Wonder, wonder",
	"Where the summer went",
	"Wonder, wonder",
	"All the time we spent",
	"Wonder, wonder",
	"♪",
];

pub struct Device {
	pub name: &'static str,
	pub kind: DeviceKind,
}

#[derive(Clone, Copy)]
pub enum DeviceKind {
	Computer,
	Phone,
	Speaker,
	Tv,
}

pub const DEVICES: [Device; 4] = [
	Device { name: "turntable", kind: DeviceKind::Computer },
	Device { name: "Khanh's iPhone", kind: DeviceKind::Phone },
	Device { name: "Living Room", kind: DeviceKind::Speaker },
	Device { name: "Bedroom TV", kind: DeviceKind::Tv },
];

pub const GENRES: [(&str, u32); 12] = [
	("Pop", 0x148a08),
	("Indie", 0xe91429),
	("K-pop", 0xe1118c),
	("Hip-Hop", 0xbc5900),
	("Chill", 0x7d4b32),
	("Jazz", 0x1e3264),
	("R&B", 0xdc148c),
	("Rock", 0xe8115b),
	("Focus", 0x503750),
	("Sleep", 0x1e3264),
	("Workout", 0x777777),
	("Soul", 0x8d67ab),
];
