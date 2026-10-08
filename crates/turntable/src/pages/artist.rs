//! An artist: banner, popular tracks, discography and the shelves around it, and about.

use gpui_kit::{
	Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
	StyledImage as _, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px, rgb,
};
use turntable_core::{Msg, Route, library::Tile};

use crate::{
	components::{Column, grouped},
	shell::Shell,
	theme,
	widgets::{chip, outline_button, section_title},
};

/// Popular shows this many rows until "See more".
const POPULAR: usize = 5;

impl Shell {
	#[expect(clippy::too_many_lines, reason = "keep the declarative artist page sections together")]
	pub fn artist_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let page = div().id("artist").size_full().overflow_y_scroll();
		let Some(l) = self.st().library.listing.clone() else {
			return page;
		};
		let route = self.st().library.route.clone();
		let o = l.overview.as_ref();
		let tint = Self::tint(l.accent.as_deref().or(o.and_then(|o| o.color.as_deref())));
		let listeners = o.and_then(|o| o.monthly_listeners);
		let avatar = o.and_then(|o| o.avatar.clone()).or(l.cover_url.clone());
		let meta = div().when_some(listeners, |s, n| s.child(format!("{} monthly listeners", grouped(n)))).into_any_element();
		let follow = outline_button("follow", if l.followed == Some(true) { "Following" } else { "Follow" })
			.on_click(cx.listener(|this, _, _, cx| this.send(Msg::ToggleFollow, cx)));
		let open = self.st().open_shelves.contains("popular");
		let shown = if open { l.tracks.len() } else { l.tracks.len().min(POPULAR) };
		let show_plays = self.st().prefs.display.play_counts;
		let popular: Vec<_> = l
			.tracks
			.iter()
			.take(shown)
			.enumerate()
			.map(|(i, t)| {
				let plays = if show_plays { l.plays.get(i).copied() } else { None };
				self.track_row(i, t, Column::Plays, plays, true, cx)
			})
			.collect();
		let mut shelves: Vec<Div> = Vec::new();
		// Discography: one shelf, its kinds (albums, singles…) as chips like Spotify's.
		if !l.discography.is_empty() {
			let pick = self.st().discography.min(l.discography.len() - 1);
			let chips = div().flex().gap_2().px(px(12.)).children(l.discography.iter().enumerate().map(|(i, (label, _))| {
				chip(("disco", i), *label, i == pick, cx.listener(move |this, _, _, cx| this.send(Msg::Discography(i), cx)))
			}));
			let cards = l.discography.get(pick).map_or_else(Vec::new, |(_, tiles)| self.cards("disc", tiles, cx));
			shelves.push(
				div()
					.flex()
					.flex_col()
					.gap_3()
					.child(div().px(px(12.)).child(section_title("Discography")))
					.child(chips)
					.child(div().h(px(theme::CARD_W + 70.)).overflow_hidden().flex().flex_wrap().children(cards)),
			);
		}
		if !l.appears_on.is_empty() {
			let cards = self.cards("appears", &l.appears_on, cx);
			shelves.push(self.shelf("Appears on", cards));
		}
		if let Some(o) = o {
			let fans: Vec<Tile> = o
				.related
				.iter()
				.map(|r| Tile {
					route: Route::Artist(r.id.clone()),
					play:  Some(format!("spotify:artist:{}", r.id)),
					title: r.name.clone(),
					sub:   "Artist".into(),
					image: r.image.clone(),
				})
				.collect();
			if !fans.is_empty() {
				let cards = self.cards("fans", &fans, cx);
				shelves.push(self.shelf("Fans also like", cards));
			}
			let found: Vec<Tile> = o
				.discovered_on
				.iter()
				.map(|p| Tile {
					route: Route::Playlist(p.id.clone()),
					play:  Some(format!("spotify:playlist:{}", p.id)),
					title: p.name.clone(),
					sub:   if p.owner.is_empty() { "Playlist".into() } else { format!("By {}", p.owner) },
					image: p.image.clone(),
				})
				.collect();
			if !found.is_empty() {
				let cards = self.cards("found", &found, cx);
				shelves.push(self.shelf("Discovered on", cards));
			}
		}
		let about = l.about.as_ref().and_then(|a| a.summary.clone().or(a.description.clone())).or(o.and_then(|o| o.biography.clone()));
		let header_img = o.and_then(|o| o.header.clone().or(o.gallery.first().cloned()));
		let more_than = l.tracks.len() > POPULAR;
		// The name, from whichever source answered first (the listing's, the artist's, the overview's).
		let name = Some(l.title.clone())
			.filter(|t| !t.is_empty())
			.or_else(|| l.artist.as_ref().map(|a| a.name.clone()))
			.or_else(|| self.known_artist_name(&route))
			.unwrap_or_default();
		page.child(self.page_header(&route, avatar.as_deref(), tint, "Artist".into(), name.into(), meta, cx))
			.child(
				self.wash(tint).child(self.action_bar(l.context_uri.clone(), follow, cx)).child(
					div()
						.px_6()
						.flex()
						.flex_col()
						.gap_2()
						.child(section_title("Popular"))
						.when(l.tracks.is_empty() && !l.popular_loaded, |s| {
							s.child(div().h(px(5. * 56.)).text_color(theme::subdued()).child("Loading…"))
						})
						.children(popular)
						.when(more_than, |s| {
							s.child(
								div()
									.id("see-more")
									.pl(px(16.))
									.pt_2()
									.text_size(px(14.))
									.font_weight(FontWeight::BOLD)
									.text_color(theme::subdued())
									.hover(|s| s.text_color(theme::text()))
									.cursor_pointer()
									.on_click(cx.listener(|this, _, _, cx| this.send(Msg::ToggleShelf("popular"), cx)))
									.child(if open { "Show less" } else { "See more" }),
							)
						}),
				),
			)
			.child(div().px_3().pt_8().flex().flex_col().gap_8().children(shelves))
			.when_some(about, |s, text| {
				let img = header_img.as_deref().and_then(|u| self.image(Some(u), 672, cx));
				s.child(
					div().px_6().pt_8().pb_10().flex().flex_col().gap_4().child(section_title("About")).child(
						div()
							.max_w(px(672.))
							.rounded(px(8.))
							.overflow_hidden()
							.bg(theme::raised())
							.child(match img {
								Some(src) => div()
									.h(px(320.))
									.child(gpui_kit::img(src).size_full().object_fit(gpui_kit::ObjectFit::Cover).rounded_t(px(8.))),
								None => div().h(px(120.)).bg(linear_gradient(
									180.,
									linear_color_stop(theme::alpha(rgb(tint), 0.6), 0.),
									linear_color_stop(theme::raised(), 1.),
								)),
							})
							.child(
								div()
									.p_6()
									.flex()
									.flex_col()
									.gap_2()
									.when_some(listeners, |s, n| {
										s.child(
											div()
												.text_size(px(16.))
												.font_weight(FontWeight::BOLD)
												.child(format!("{} monthly listeners", grouped(n))),
										)
									})
									.child(div().text_size(px(16.)).text_color(theme::subdued()).line_clamp(4).child(text)),
							),
					),
				)
			})
	}

	/// Cards for `tiles`, ids prefixed with `key`: only as many as fit one row of the main view
	/// (shelves don't scroll sideways, so the rest would be built and laid out for nothing).
	pub fn cards(&self, key: &'static str, tiles: &[Tile], cx: &mut Context<'_, Self>) -> Vec<gpui_kit::Stateful<Div>> {
		#[expect(
			clippy::cast_possible_truncation,
			clippy::cast_sign_loss,
			reason = "a small positive card count (max(1.) floors it at one)"
		)]
		let fit = ((self.main_w - 24.) / theme::CARD_W).floor().max(1.) as usize;
		tiles.iter().take(fit).enumerate().map(|(i, t)| self.card(gpui_kit::ElementId::NamedInteger(key.into(), i as u64), t, cx)).collect()
	}
}

impl Shell {
	/// An artist's name from what's already at hand: followed artists, artists seen in rows,
	/// or the now-playing track's credits.
	fn known_artist_name(&self, route: &Route) -> Option<String> {
		let Route::Artist(id) = route else { return None };
		let st = self.st();
		let uri = format!("spotify:artist:{id}");
		st.library
			.artists
			.get(id)
			.map(|a| a.name.clone())
			.or_else(|| st.library.followed.iter().find(|a| a.uri == uri).map(|a| a.name.clone()))
			.or_else(|| st.item.decorations.creators.iter().find(|c| c.entity.uri == uri).map(|c| c.entity.name().to_owned()))
	}
}
