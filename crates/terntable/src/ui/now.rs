//! The now-playing view: the cover, the track, its artist, who made it and what plays next.
//! A side panel in wide layouts; in the compact layout a full page that also
//! carries the controls. The artist's listeners and the credits are the web
//! player's (`spotify::WebPlayer`), so they appear once it answers; the lyrics
//! have a panel of their own.

use tern_sdk::ui::{Node, html};

use super::{Tap, art, button, devices, div, like_button, open_spotify, seek, text, transport, volume};
use crate::{
	app::{Layout, Msg, Panel, State},
	engine::CoverSize,
	library::Route,
	spotify::{self, id_of},
	ui::icons::{Glyph, Icons},
};

pub fn panel(state: &State, icons: &Icons) -> Node<Msg> {
	let item = &state.item;
	let compact = state.layout() == Layout::Compact;
	// What it plays from (a playlist, an album, an artist), as Spotify heads this view.
	let heading = [state.context.name(), item.parent_name()].into_iter().find(|n| !n.is_empty()).unwrap_or("Now playing");
	let mut head = div("tt-nowview-top").key("top");
	if compact {
		head = head.child(button(icons, Glyph::Down, "Close", Msg::TogglePanel(Panel::NowPlaying), "", 22.0));
	}
	head = head.child(text("tt-nowview-head tt-trunc", heading.to_owned()));
	if compact {
		head = head.child(devices::button(state, icons, 20.0));
	}
	let mut panel = div(if compact { "tt-panel tt-nowview tt-nowview-full" } else { "tt-panel tt-nowview" })
		.key("now")
		.child(head)
		.child(art(state, item.cover(CoverSize::Large), "tt-nowview-cover", item.parent_name(), Tap::NONE));

	let mut title = div("tt-nowview-title").child(
		div("tt-tname-wrap")
			.child(text("tt-nowview-name tt-trunc", item.name().to_owned()))
			.child(text("tt-dim tt-trunc", item.creators())),
	);
	if let Some(like) = like_button(state, icons, 22.0) {
		title = title.child(like);
	}
	panel = panel.child(title);
	if compact {
		panel = panel.child(seek(state)).child(transport(state, icons, false)).child(
			div("tt-nowview-tools")
				.child(volume(state, icons))
				.child(button(icons, Glyph::Mic, "Lyrics", Msg::TogglePanel(Panel::Lyrics), "", 20.0))
				.child(button(icons, Glyph::Queue, "Queue", Msg::TogglePanel(Panel::Queue), "", 20.0))
				.child(button(icons, Glyph::Sliders, "Equalizer", Msg::TogglePanel(Panel::Eq), "", 20.0))
				.child(button(icons, Glyph::Gear, "Settings", Msg::TogglePanel(Panel::Settings), "", 20.0)),
		);
	}

	if let Some(about) = about_artist(state) {
		panel = panel.child(about);
	}
	if let Some(credits) = credits(state) {
		panel = panel.child(credits);
	}
	panel = panel.child(next_in_queue(state));
	if let Some(href) = super::spotify_href(&item.uri) {
		panel = panel.child(div("tt-attrib").child(open_spotify(icons, href)));
	}
	panel.into()
}

/// The track's first artist, once the library worker fetched it: the header photo with the
/// label over it, then the name, listeners and a follow button, and the artist's own words
/// (when the web player answered; else the genres).
fn about_artist(state: &State) -> Option<Node<Msg>> {
	let id = id_of(&state.item.decorations.creators.first()?.entity.uri);
	let a = state.library.artists.get(id)?;
	let overview = state.library.overviews.get(id);
	let go = Msg::Navigate(Route::Artist(id.to_owned()));
	let image = overview
		.and_then(|o| o.header.as_deref().or(o.avatar.as_deref()))
		.or_else(|| spotify::pick_image(&a.images, 640).map(|i| i.url.as_str()));
	let top = div("tt-about-top")
		.child(art(state, image, "tt-about-img", &a.name, Tap::Msg(go.clone(), None)))
		.child(text("tt-nowview-sub tt-about-label", "About the artist"));
	let mut name = div("tt-about-nameline").child(
		html::div().class("tt-about-name tt-about-link tt-trunc").title(format!("Open {}", a.name)).text(a.name.clone()).on_click(go),
	);
	if overview.is_some_and(|o| o.verified) {
		name = name.child(html::span("✓").class("tt-verified").title("Verified artist"));
	}
	let mut info = div("tt-about-info").child(name);
	if let Some(n) = overview.and_then(|o| o.monthly_listeners) {
		info = info.child(
			div("tt-about-statrow")
				.child(div("tt-about-stat tt-dim").child(text("tt-num", super::grouped(n))).child(text("", "monthly listeners")))
				.child(follow_button(state, id, &a.name)),
		);
	}
	if let Some(bio) = overview.and_then(|o| o.biography.as_ref()) {
		info = info.child(text("tt-dim tt-about-bio", bio.clone()));
	} else if !a.genres.is_empty() {
		let genres = a.genres.iter().take(3).map(String::as_str).collect::<Vec<_>>().join(", ");
		info = info.child(text("tt-dim tt-about-genres", genres));
	}
	Some(div("tt-nowview-card tt-about").child(top).child(info).into())
}

/// "Follow" / "Following" for the artist `id`.
fn follow_button(state: &State, id: &str, name: &str) -> Node<Msg> {
	let uri = format!("spotify:artist:{id}");
	let followed = state.library.followed.iter().any(|a| a.uri == uri);
	html::el("button")
		.class(if followed { "tt-pill tt-quiet tt-on" } else { "tt-pill tt-quiet" })
		.text(if followed { "Following" } else { "Follow" })
		.on_click(Msg::FollowArtist(id.to_owned(), name.to_owned()))
		.into()
}

/// Who made the playing song: the first few, then all.
fn credits(state: &State) -> Option<Node<Msg>> {
	let (uri, credits) = state.library.credits.as_ref()?;
	if *uri != state.item.uri || credits.is_empty() {
		return None;
	}
	let open = state.open_shelves.contains("credits");
	let mut head = div("tt-shelf-head").child(text("tt-nowview-sub", "Credits"));
	if credits.len() > CREDITS_SHOWN {
		head = head.child(
			html::el("button")
				.class("tt-link tt-dim")
				.text(if open { "Show less" } else { "Show all" })
				.on_click(Msg::ToggleShelf("credits")),
		);
	}
	let mut card = div("tt-nowview-card tt-credits").child(head);
	for (i, c) in credits.iter().enumerate().take(if open { usize::MAX } else { CREDITS_SHOWN }) {
		let mut row = div("tt-credit").key(format!("{i}-{}", c.name));
		row =
			row.child(div("tt-credit-text").child(text("tt-credit-name", c.name.clone())).child(text("tt-dim tt-trunc", c.roles.clone())));
		if let Some(id) = &c.id {
			row = row.child(follow_button(state, id, &c.name));
		}
		card = card.child(row);
	}
	Some(card.into())
}

/// Credits shown before "Show all".
const CREDITS_SHOWN: usize = 3;

fn next_in_queue(state: &State) -> Node<Msg> {
	let head = div("tt-shelf-head")
		.child(text("tt-nowview-sub", "Next in queue"))
		.child(html::el("button").class("tt-link tt-dim").text("Open queue").on_click(Msg::TogglePanel(Panel::Queue)));
	let row: Node<Msg> = match state.upcoming.first() {
		Some(e) => div("tt-qrow")
			.child(art(state, e.item.cover(CoverSize::Small), "tt-art-40", e.item.name(), Tap::NONE))
			.child(
				div("tt-tname-wrap")
					.child(text("tt-tname tt-trunc", e.item.name().to_owned()))
					.child(text("tt-tsub tt-trunc tt-dim", e.item.creators())),
			)
			.into(),
		None => text("tt-dim", "Nothing queued."),
	};
	div("tt-nowview-card tt-next").child(head).child(row).into()
}
