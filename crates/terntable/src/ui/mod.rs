//! Views. Everything that moves while music plays (the seek bar, the elapsed
//! clock) is clocked by Tern itself: terntable renders only when something
//! changes. One client that rearranges itself for the pane's size
//! ([`Layout`]): resizing never switches to a different app.

mod bar;
mod browse;
mod devices;
mod dialog;
mod eq;
pub(crate) mod icons;
mod lyrics;
mod now;
mod settings;
mod setup;
mod sheet;

pub use eq::send_curve as send_eq_curve;
pub use lyrics::{line_id, sheet as lyrics_sheet};
pub use sheet::SHEET;
use tern_sdk::{
	View,
	ui::{self, ElapsedFormat, Node, html},
};

use crate::{
	app::{self, Layout, Msg, SEEK_CELLS, State, VOLUME_CELLS},
	library::Route,
	ui::icons::{Glyph, Icons},
};

pub fn view(state: &State, icons: &Icons) -> View<Msg> {
	// A region's root node is the region itself (Tern drops its class), so each
	// layout hangs off a plain root.
	let view = match state.layout() {
		Layout::Wide | Layout::Medium => {
			let mut dock = ui::col().child(bar::bar(state, icons));
			if let Some(strip) = devices::strip(state, icons) {
				dock = dock.child(strip);
			}
			View::new().main(ui::col().child(browse::app(state, icons))).dock(dock)
		},
		Layout::Compact => {
			// The now-playing page has the controls: no mini player under it.
			let mut dock = ui::col();
			if state.panel != Some(app::Panel::NowPlaying) {
				dock = dock.child(bar::mini(state, icons));
			}
			if let Some(strip) = devices::strip(state, icons) {
				dock = dock.child(strip);
			}
			View::new().main(ui::col().child(browse::app(state, icons))).dock(dock.child(bar::tabs(state, icons)))
		},
		// A full-size overlay in `layer` is the one box covering the whole
		// surface (`main` always leaves a strip for the dock under it).
		Layout::Strip => {
			let strip = ui::overlay().size(ui::OverlaySize::Full).role("tt.strip").child(bar::mini(state, icons));
			return View::new().main(ui::col()).layer(ui::col().child(strip));
		},
	};
	let mut layer = ui::col();
	if let Some(d) = &state.dialog {
		layer = layer.child(dialog::dialog(state, icons, d));
	}
	if state.library_menu && state.library_tools() {
		layer = layer.child(browse::library_menu(state, icons));
	}
	if let Some(menu) = settings::dropdown(state, icons) {
		layer = layer.child(menu);
	}
	if state.devices_menu_open() {
		layer = layer.child(devices::menu(state, icons));
	}
	layer = layer.children(eq::menu(state, icons));
	if let Some((n, text)) = &state.toast {
		layer = layer.child(ui::toast(text.clone()).key(format!("toast{n}")));
	}
	view.layer(layer)
}

/// Sidebar width in CSS px, open and collapsed (matches `.tt-app` in the sheet).
const SIDEBAR_PX: f64 = 240.0;
const SIDEBAR_COLLAPSED_PX: f64 = 72.0;
/// The right-hand panel's width plus its gap (matches `.tt-with-queue`).
const PANEL_PX: f64 = 308.0;

/// Card columns of the content area: its width over a 148 px minimum card
/// (zoom ≈ 1.08 at 14 px fonts). Home shelves show one row of this many.
pub fn card_columns(state: &State) -> usize {
	let (pane_w, _) = state.pane_px();
	if pane_w <= 0.0 {
		return 4;
	}
	let side = match state.layout() {
		Layout::Compact | Layout::Strip => 0.0,
		_ if state.sidebar_collapsed() => SIDEBAR_COLLAPSED_PX,
		_ => SIDEBAR_PX,
	};
	let panel = if state.side_panel().is_some() { PANEL_PX } else { 0.0 };
	let content = pane_w / 1.08 - side - panel - 16.0 - 8.0 - 48.0;
	let columns = ((content + 8.0) / (148.0 + 8.0)).floor().clamp(2.0, 8.0);
	#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 2..=8 above")]
	let columns = columns as usize;
	columns
}

/// The color the open page's header fades from (an artist's photo, a cover): `#rrggbb`.
pub fn tint(state: &State) -> Option<&str> {
	let l = state.library.listing.as_ref()?;
	if matches!(state.library.route, Route::Artist(_) | Route::Album(_) | Route::Playlist(_)) { l.accent.as_deref() } else { None }
}

/// The small sheet that changes at runtime: the seek bar's animation
/// (restarted by alternating keyframe names), the volume fill, the grids'
/// column counts and the page tint.
pub fn dynamic_sheet(state: &State, flip: bool) -> String {
	let mut css = format!(".tt-vfill {{ width: {}%; }}", state.volume.min(100));
	if let Some(tint) = tint(state) {
		css.push_str(&format!(" .tt-tinted {{ --tint: {tint}; }}"));
	}
	let n = card_columns(state);
	css.push_str(&format!(" .tt-cards {{ grid-template-columns: repeat({n}, minmax(0, 1fr)); }}"));
	// The quick-pick grid: four across when there's room for six cards, else two.
	let quick = if n >= 6 { 4 } else { 2 };
	css.push_str(&format!(" .tt-quick-grid {{ grid-template-columns: repeat({quick}, minmax(0, 1fr)); }}"));
	let duration = state.duration_ms();
	if duration == 0 {
		css.push_str(" .tt-fill { transform: scaleX(0); }");
		return css;
	}
	let position = state.position_ms().min(duration);
	// Another keyframe name makes Tern start the animation over (the same name keeps it running).
	let name = if flip { "tt-p1" } else { "tt-p0" };
	let run = if state.playing() { "running" } else { "paused" };
	css.push_str(&format!(
		" @keyframes tt-p0 {{ from {{ transform: scaleX(0) }} to {{ transform: scaleX(1) }} }}\
		 @keyframes tt-p1 {{ from {{ transform: scaleX(0) }} to {{ transform: scaleX(1) }} }}\
		 .tt-fill {{ animation: {name} {duration}ms linear -{position}ms 1 forwards; animation-play-state: {run}; }}\
		 .sf-still .tt-fill {{ animation-play-state: {run}; }}"
	));
	css
}

// ----------------------------------------------------------------- helpers

fn div(class: &str) -> html::El<Msg> {
	html::div().class(class)
}

fn text(class: &str, text: impl Into<String>) -> Node<Msg> {
	html::div().class(class).text(text).into()
}

/// What a click on a picture does. Tern opens a blob image in its zoom viewer
/// unless the image has its own click action, and the image takes the click
/// before its parent does, so every picture names one.
enum Tap {
	/// Send these (click, double click), usually the parent's own.
	Msg(Msg, Option<Msg>),
	/// Open this link (a cover that links back to Spotify).
	Href(String),
}

impl Tap {
	/// A picture that does nothing on its own.
	const NONE: Self = Self::Msg(Msg::Noop, None);
}

fn picture(image: ui::Image<Msg>, tap: Tap) -> ui::Image<Msg> {
	match tap {
		Tap::Msg(click, None) => image.on_click(click),
		Tap::Msg(click, Some(dbl)) => image.on_click(click).on_dblclick(dbl),
		Tap::Href(href) => image.on_click(Msg::OpenUrl(href)),
	}
}

fn icon(icons: &Icons, glyph: Glyph, px: f64, tap: Tap) -> Node<Msg> {
	picture(ui::image().blob(icons.id(glyph).to_owned()).w(px).h(px).role("tt.icon"), tap).into()
}

fn button(icons: &Icons, glyph: Glyph, label: &str, msg: Msg, class: &str, px: f64) -> Node<Msg> {
	html::el("button")
		.class(if class.is_empty() { "tt-btn".to_owned() } else { format!("tt-btn {class}") })
		.title(label)
		.aria(label)
		.child(icon(icons, glyph, px, Tap::Msg(msg.clone(), None)))
		.on_click(msg)
		.into()
}

/// Square artwork of a fixed size class (`tt-art-40`, `tt-art-56`, `tt-art-180`).
fn art(state: &State, url: Option<&str>, size: &str, alt: &str, tap: Tap) -> Node<Msg> {
	match state.image(url) {
		// Loaded: no placeholder fill, or it shows through the clipped corners' edge.
		Some(blob) => html::div()
			.class(format!("tt-art tt-loaded {size}"))
			.child(picture(ui::image().blob(blob.to_owned()).alt(alt.to_owned()).role("tt.art"), tap))
			.into(),
		None => html::div().class(format!("tt-art {size}")).into(),
	}
}

/// Shuffle · previous · play/pause · next · repeat.
fn transport(state: &State, icons: &Icons, compact: bool) -> Node<Msg> {
	use crate::engine::Repeat;
	let (play, play_label) = if state.playing() { (Glyph::Pause, "Pause") } else { (Glyph::Play, "Play") };
	let (repeat, repeat_label) = match state.options.repeat {
		Repeat::Off => (Glyph::Repeat, "Enable repeat"),
		Repeat::Context => (Glyph::Repeat, "Enable repeat one"),
		Repeat::Track => (Glyph::RepeatOne, "Disable repeat"),
	};
	let on = |active: bool| if active { "tt-on" } else { "" };
	let shuffle_label = if state.options.shuffle { "Disable shuffle" } else { "Enable shuffle" };
	let mut row = div("tt-controls");
	if !compact {
		row = row.child(button(icons, Glyph::Shuffle, shuffle_label, Msg::ToggleShuffle, on(state.options.shuffle), 18.0));
	}
	row = row
		.child(button(icons, Glyph::Prev, "Previous", Msg::Prev, "", 18.0))
		.child(button(icons, play, play_label, Msg::TogglePlay, "tt-primary", 20.0))
		.child(button(icons, Glyph::Next, "Next", Msg::Next, "", 18.0));
	if !compact {
		row = row.child(button(icons, repeat, repeat_label, Msg::CycleRepeat, on(state.options.repeat != Repeat::Off), 18.0));
	}
	row.into()
}

/// Elapsed · clickable rail · duration.
fn seek(state: &State) -> Node<Msg> {
	let duration = state.duration_ms();
	let position = state.position_ms();
	let mut clock = ui::elapsed().format(ElapsedFormat::Clock).age(position.cast_signed());
	if !state.playing() || duration == 0 {
		clock = clock.stopped(position);
	}
	let mut hit = div("tt-hit");
	if duration > 0 {
		for cell in 0..SEEK_CELLS {
			hit = hit.child(html::span("").class("tt-cell").key(cell).on_click(Msg::SeekCell(cell)));
		}
	}
	div("tt-seek tt-num tt-dim")
		.child(html::span("").child(clock))
		.child(div("tt-rail").child(div("tt-track").child(div("tt-fill"))).child(hit))
		.child(html::span(if duration > 0 { app::clock(duration) } else { "0:00".to_owned() }))
		.into()
}

fn volume(state: &State, icons: &Icons) -> Node<Msg> {
	let (glyph, label) = if state.volume == 0 { (Glyph::Mute, "Unmute") } else { (Glyph::Volume, "Mute") };
	let mut hit = div("tt-hit");
	for cell in 0..VOLUME_CELLS {
		hit = hit.child(html::span("").class("tt-cell").key(cell).on_click(Msg::VolumeCell(cell + 1)));
	}
	// Some devices set their own volume: the control stays, dimmed.
	let fixed = state.remote().is_some_and(|d| !d.volume_capable());
	div(if fixed { "tt-vol tt-inert" } else { "tt-vol" })
		.child(button(icons, glyph, label, Msg::ToggleMute, "", 18.0))
		.child(div("tt-vrail").title(format!("Volume {}%", state.volume)).child(div("tt-track").child(div("tt-vfill"))).child(hit))
		.into()
}

/// Spotify's save button, once the track's state is known (no flicker of a wrong
/// state): ⊕ saves to Liked Songs; once saved, the ✓ opens "Save in" (Liked Songs
/// and the user's playlists), so one control covers both.
fn like_button(state: &State, icons: &Icons, px: f64) -> Option<Node<Msg>> {
	let picker = state.item.uri.starts_with("spotify:track:") && state.library.conn == crate::library::Conn::Ready;
	let (glyph, label, class, msg) = if !state.liked()? {
		(Glyph::Save, "Save to Liked Songs", "tt-like", Msg::ToggleLike)
	} else if picker {
		(Glyph::Saved, "Save in…", "tt-like tt-on", Msg::AddNowPlayingToPlaylist)
	} else {
		(Glyph::Saved, "Remove from Liked Songs", "tt-like tt-on", Msg::ToggleLike)
	};
	Some(button(icons, glyph, label, msg, class, px))
}

/// The link back to Spotify that its guidelines ask for, with Spotify's icon.
fn open_spotify(icons: &Icons, href: String) -> Node<Msg> {
	html::div()
		.class("tt-open-spotify")
		.title("Open in the Spotify app or web player")
		.child(icon(icons, Glyph::Spotify, 21.0, Tap::Href(href.clone())))
		.child(html::span("OPEN SPOTIFY"))
		.on_click(Msg::OpenUrl(href))
		.into()
}

/// Artists of a Web API track, joined.
fn artists(track: &crate::spotify::Track) -> String {
	track.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// `spotify:track:ID` → `https://open.spotify.com/track/ID` (policy: link back to Spotify).
pub fn spotify_href(uri: &str) -> Option<String> {
	let mut parts = uri.splitn(3, ':');
	match (parts.next(), parts.next(), parts.next()) {
		(Some("spotify"), Some(kind), Some(id)) if !kind.is_empty() && !id.is_empty() => {
			Some(format!("https://open.spotify.com/{kind}/{id}"))
		},
		_ => None,
	}
}
/// 3890683 → "3,890,683".
fn grouped(n: u64) -> String {
	let digits = n.to_string();
	let mut out = String::with_capacity(digits.len() + digits.len() / 3);
	for (i, c) in digits.chars().enumerate() {
		if i > 0 && (digits.len() - i).is_multiple_of(3) {
			out.push(',');
		}
		out.push(c);
	}
	out
}
