//! terntable's icons: one 24 px grid, 2 px round strokes, drawn in
//! `currentColor` so they follow the theme and the button's state.
//! Each is sent once per session; views refer to them by blob id.

use tern_sdk::{Session, Surface};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
	Play,
	Pause,
	Prev,
	Next,
	Shuffle,
	Repeat,
	RepeatOne,
	Volume,
	Mute,
	/// The devices button (Spotify Connect).
	Device,
	/// A device in the devices menu, by type.
	Computer,
	Phone,
	Speaker,
	Tv,
	Home,
	Search,
	/// Liked Songs' artwork.
	Heart,
	/// Add to Liked Songs, and its saved state.
	Save,
	Saved,
	Queue,
	Check,
	Sliders,
	/// Settings.
	Gear,
	/// Your Library, at rest.
	Library,
	/// Your Library's title on hover: the sidebar panel with the way it would move.
	SidebarOpen,
	SidebarClose,
	/// The now-playing view (right-hand panel).
	NowPlaying,
	/// Lyrics.
	Mic,
	/// Close a full-screen view.
	Down,
	/// Create.
	Plus,
	/// Close a field.
	Close,
	/// Playlist details and visibility.
	Pencil,
	Globe,
	Lock,
	/// Your Library's views: a compact list, a list, a grid and a large grid.
	ViewCompact,
	ViewList,
	ViewGrid,
	ViewLarge,
	/// Spotify's icon, for attribution (in Spotify's green).
	Spotify,
	/// The sleep timer.
	Moon,
}

const ALL: [Glyph; 40] = [
	Glyph::Play,
	Glyph::Pause,
	Glyph::Prev,
	Glyph::Next,
	Glyph::Shuffle,
	Glyph::Repeat,
	Glyph::RepeatOne,
	Glyph::Volume,
	Glyph::Mute,
	Glyph::Device,
	Glyph::Computer,
	Glyph::Phone,
	Glyph::Speaker,
	Glyph::Tv,
	Glyph::Home,
	Glyph::Search,
	Glyph::Heart,
	Glyph::Save,
	Glyph::Saved,
	Glyph::Queue,
	Glyph::Check,
	Glyph::Sliders,
	Glyph::Gear,
	Glyph::Library,
	Glyph::SidebarOpen,
	Glyph::SidebarClose,
	Glyph::NowPlaying,
	Glyph::Mic,
	Glyph::Down,
	Glyph::Plus,
	Glyph::Close,
	Glyph::Pencil,
	Glyph::Globe,
	Glyph::Lock,
	Glyph::ViewCompact,
	Glyph::ViewList,
	Glyph::ViewGrid,
	Glyph::ViewLarge,
	Glyph::Spotify,
	Glyph::Moon,
];

const STROKE: &str = r#"fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round""#;

fn icon(body: &str) -> String {
	format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" {STROKE}>{body}</svg>"#)
}

fn svg(glyph: Glyph) -> String {
	match glyph {
		Glyph::Play => icon(r#"<path d="M8 5.2v13.6a1 1 0 0 0 1.53.85l10.9-6.8a1 1 0 0 0 0-1.7L9.53 4.35A1 1 0 0 0 8 5.2z" fill="currentColor" stroke="none"/>"#),
		Glyph::Pause => icon(
			r#"<rect x="6" y="4.5" width="4" height="15" rx="1.2" fill="currentColor" stroke="none"/><rect x="14" y="4.5" width="4" height="15" rx="1.2" fill="currentColor" stroke="none"/>"#,
		),
		Glyph::Prev => icon(
			r#"<path d="M19 6.1v11.8a1 1 0 0 1-1.55.83L9.3 13.24a1.5 1.5 0 0 1 0-2.48l8.15-5.49A1 1 0 0 1 19 6.1z" fill="currentColor" stroke="none"/><path d="M6 5.5v13"/>"#,
		),
		Glyph::Next => icon(
			r#"<path d="M5 6.1v11.8a1 1 0 0 0 1.55.83l8.15-5.49a1.5 1.5 0 0 0 0-2.48L6.55 5.27A1 1 0 0 0 5 6.1z" fill="currentColor" stroke="none"/><path d="M18 5.5v13"/>"#,
		),
		Glyph::Shuffle => icon(
			r#"<path d="M3 7h2.6c1.6 0 2.7.7 3.6 2l4.6 6c.9 1.3 2 2 3.6 2H20"/><path d="M3 17h2.6c1.6 0 2.7-.7 3.6-2"/><path d="M14.8 9c.9-1.3 2-2 3.6-2H20"/><path d="M17.5 4.5 20 7l-2.5 2.5"/><path d="M17.5 14.5 20 17l-2.5 2.5"/>"#,
		),
		Glyph::Repeat => icon(
			r#"<path d="M4 11.5V10a3.5 3.5 0 0 1 3.5-3.5H19"/><path d="M16.5 4 19 6.5 16.5 9"/><path d="M20 12.5V14a3.5 3.5 0 0 1-3.5 3.5H5"/><path d="M7.5 20 5 17.5 7.5 15"/>"#,
		),
		Glyph::RepeatOne => icon(
			r#"<path d="M4 11.5V10a3.5 3.5 0 0 1 3.5-3.5H19"/><path d="M16.5 4 19 6.5 16.5 9"/><path d="M20 12.5V14a3.5 3.5 0 0 1-3.5 3.5H5"/><path d="M7.5 20 5 17.5 7.5 15"/><path d="M11.2 10.6 12.4 10v4" stroke-width="1.6"/>"#,
		),
		Glyph::Volume => icon(
			r#"<path d="M4 9.5h2.8L11 6v12l-4.2-3.5H4a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1z" fill="currentColor" stroke="none"/><path d="M15 9.2a4 4 0 0 1 0 5.6"/><path d="M17.8 6.5a8 8 0 0 1 0 11"/>"#,
		),
		Glyph::Mute => icon(
			r#"<path d="M4 9.5h2.8L11 6v12l-4.2-3.5H4a1 1 0 0 1-1-1v-3a1 1 0 0 1 1-1z" fill="currentColor" stroke="none"/><path d="m15.5 9.5 5 5"/><path d="m20.5 9.5-5 5"/>"#,
		),
		// Spotify's Connect button: a screen with a speaker in front of it.
		Glyph::Device => icon(
			r#"<path d="M13 17H5a1.5 1.5 0 0 1-1.5-1.5v-9A1.5 1.5 0 0 1 5 5h14a1.5 1.5 0 0 1 1.5 1.5V9"/><path d="M7.5 20.5H13"/><rect x="14.5" y="11" width="7" height="10" rx="1.6"/><circle cx="18" cy="16.5" r="1.7"/>"#,
		),
		Glyph::Computer => icon(r#"<rect x="4" y="5" width="16" height="11" rx="1.5"/><path d="M2.5 19.5h19"/>"#),
		Glyph::Phone => icon(r#"<rect x="7" y="2.5" width="10" height="19" rx="2.5"/><path d="M11 18.5h2"/>"#),
		Glyph::Speaker => icon(
			r#"<rect x="6" y="2.5" width="12" height="19" rx="2.5"/><circle cx="12" cy="14.5" r="3.5"/><circle cx="12" cy="7" r="1" fill="currentColor" stroke="none"/>"#,
		),
		Glyph::Tv => icon(r#"<rect x="3" y="4.5" width="18" height="12" rx="2"/><path d="M8 20.5h8"/><path d="M12 16.5v4"/>"#),
		Glyph::Home => icon(r#"<path d="M4 10.5 12 4l8 6.5V19a1 1 0 0 1-1 1h-4.5v-5.5h-5V20H5a1 1 0 0 1-1-1z"/>"#),
		Glyph::Search => icon(r#"<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.2-4.2"/>"#),
		Glyph::Save => icon(r#"<circle cx="12" cy="12" r="9"/><path d="M12 8v8"/><path d="M8 12h8"/>"#),
		Glyph::Saved => icon(
			r##"<circle cx="12" cy="12" r="10" fill="#1ed760" stroke="none"/><path d="m7.8 12.3 2.9 2.9 5.6-5.8" stroke="#000" stroke-width="2.2"/>"##,
		),
		Glyph::Heart => icon(
			// Lobes top out at y≈5.6 and the tip is at 20: lifted 0.8 to sit on the grid's centre.
			r#"<path d="M12 20s-7.5-4.4-7.5-10A4.4 4.4 0 0 1 12 7.2 4.4 4.4 0 0 1 19.5 10c0 5.6-7.5 10-7.5 10z" fill="currentColor" stroke="none" transform="translate(0 -0.8)"/>"#,
		),
		Glyph::Queue => icon(r#"<path d="M4 6h13"/><path d="M4 12h13"/><path d="M4 18h7"/><path d="M15.5 15v6l5-3z" fill="currentColor" stroke="none"/>"#),
		Glyph::Check => icon(r#"<path d="m5 12.5 4.5 4.5L19 7.5" stroke-width="2.4"/>"#),
		Glyph::Sliders => icon(
			r#"<path d="M6 4v5"/><path d="M6 13v7"/><path d="M12 4v9"/><path d="M12 17v3"/><path d="M18 4v2"/><path d="M18 10v10"/><circle cx="6" cy="11" r="2"/><circle cx="12" cy="15" r="2"/><circle cx="18" cy="8" r="2"/>"#,
		),
		// Two spines and a book's rounded back, centred on the 24 px grid.
		Glyph::Library => icon(r#"<path d="M4.5 4v16"/><path d="M8.5 4v16"/><path d="M12.5 4h3a4 4 0 0 1 4 4v8a4 4 0 0 1-4 4h-3z"/>"#),
		Glyph::SidebarOpen => icon(r#"<rect x="3.5" y="4" width="17" height="16" rx="2"/><path d="M9 4v16"/><path d="m13 9.5 2.5 2.5-2.5 2.5"/>"#),
		Glyph::SidebarClose => icon(r#"<rect x="3.5" y="4" width="17" height="16" rx="2"/><path d="M9 4v16"/><path d="m15.5 9.5-2.5 2.5 2.5 2.5"/>"#),
		Glyph::NowPlaying => icon(
			r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M10 9v6l5-3z" fill="currentColor" stroke="none"/>"#,
		),
		Glyph::Mic => icon(r#"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5.5 11.5a6.5 6.5 0 0 0 13 0"/><path d="M12 18v3"/><path d="M8.5 21h7"/>"#),
		Glyph::Gear => icon(
			r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>"#,
		),
		Glyph::Down => icon(r#"<path d="m6 9 6 6 6-6"/>"#),
		Glyph::Plus => icon(r#"<path d="M12 5v14"/><path d="M5 12h14"/>"#),
		Glyph::Close => icon(r#"<path d="M6 6l12 12"/><path d="M18 6 6 18"/>"#),
		Glyph::Moon => icon(r#"<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/>"#),
		Glyph::Pencil => icon(r#"<path d="m15.5 4.5 4 4L9 19l-5 1 1-5z"/><path d="m13.5 6.5 4 4"/>"#),
		Glyph::Globe => icon(r#"<circle cx="12" cy="12" r="9"/><path d="M12 3c-5.3 5-5.3 13 0 18 5.3-5 5.3-13 0-18z"/><path d="M3 12h18"/>"#),
		Glyph::Lock => icon(r#"<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/>"#),
		Glyph::ViewCompact => icon(r#"<path d="M4 7h16"/><path d="M4 12h16"/><path d="M4 17h16"/>"#),
		Glyph::ViewList => icon(
			r#"<path d="M9 7h11"/><path d="M9 12h11"/><path d="M9 17h11"/><g fill="currentColor" stroke="none"><circle cx="4.8" cy="7" r="1.2"/><circle cx="4.8" cy="12" r="1.2"/><circle cx="4.8" cy="17" r="1.2"/></g>"#,
		),
		Glyph::ViewGrid => icon(
			r#"<g fill="currentColor" stroke="none"><circle cx="6" cy="6" r="1.6"/><circle cx="12" cy="6" r="1.6"/><circle cx="18" cy="6" r="1.6"/><circle cx="6" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="18" cy="12" r="1.6"/><circle cx="6" cy="18" r="1.6"/><circle cx="12" cy="18" r="1.6"/><circle cx="18" cy="18" r="1.6"/></g>"#,
		),
		Glyph::ViewLarge => icon(
			r#"<rect x="4" y="4" width="6.5" height="6.5" rx="1.5"/><rect x="13.5" y="4" width="6.5" height="6.5" rx="1.5"/><rect x="4" y="13.5" width="6.5" height="6.5" rx="1.5"/><rect x="13.5" y="13.5" width="6.5" height="6.5" rx="1.5"/>"#,
		),
		Glyph::Spotify => r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><circle cx="12" cy="12" r="12" fill="#1ED760"/><path d="M17.5 16.6a.75.75 0 0 1-1 .25c-2.85-1.74-6.43-2.13-10.66-1.17a.75.75 0 1 1-.33-1.46c4.62-1.06 8.59-.6 11.74 1.33.35.22.47.69.25 1.05zm1.47-3.27a.94.94 0 0 1-1.29.31c-3.26-2-8.23-2.58-12.08-1.42a.94.94 0 1 1-.54-1.8c4.4-1.33 9.88-.68 13.6 1.6.44.27.58.86.31 1.31zm.13-3.4C15.18 7.6 8.75 7.38 5.03 8.51a1.13 1.13 0 1 1-.65-2.16c4.27-1.3 11.37-1.05 15.85 1.61a1.13 1.13 0 0 1-1.13 1.97z" fill="#000"/></svg>"##
			.to_owned(),
	}
}

/// Blob ids of every glyph, sent to Tern once.
pub struct Icons {
	ids: Vec<String>,
}

// `Icons::id` reads a glyph's blob id at the glyph's discriminant, so `ALL` lists the glyphs in
// declaration order, none left out.
#[expect(clippy::indexing_slicing, reason = "i < ALL.len()")]
const _: () = {
	let mut i = 0;
	while i < ALL.len() {
		assert!(ALL[i] as usize == i, "icons::ALL must list the glyphs in declaration order");
		i += 1;
	}
};

impl Icons {
	pub fn send<M>(session: &mut Session<M>, _sf: Surface) -> Result<Self, tern_sdk::Error> {
		let mut ids = Vec::with_capacity(ALL.len());
		for glyph in ALL {
			ids.push(session.blob(svg(glyph).into_bytes(), Some("image/svg+xml"))?);
		}
		Ok(Self { ids })
	}

	pub fn id(&self, glyph: Glyph) -> &str {
		// One id per entry of `ALL`, which is in declaration order (asserted above).
		#[expect(clippy::indexing_slicing, reason = "ids has one entry per ALL entry, indexed by discriminant")]
		let id = &self.ids[glyph as usize];
		id
	}
}
