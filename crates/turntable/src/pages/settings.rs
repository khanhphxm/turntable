//! Settings: account, playback, audio, display and sources, as labelled rows with a control on
//! the right.

use gpui_kit::{
	Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
	component::switch::Switch, div, px,
};
use turntable_core::{
	Msg,
	app::settings::Pref,
	audio::Normalize,
};

use crate::{
	shell::Shell,
	theme,
	widgets::{chip, outline_button},
};

/// A settings row: label and help text on the left, the control on the right.
fn setting(label: impl Into<SharedString>, help: &'static str, control: impl IntoElement) -> Div {
	div()
		.min_h(px(56.))
		.py_2()
		.flex()
		.items_center()
		.justify_between()
		.gap_6()
		.child(
			div()
				.flex()
				.flex_col()
				.gap_0p5()
				.child(div().text_size(px(16.)).child(label.into()))
				.child(div().text_size(px(14.)).text_color(theme::subdued()).child(help)),
		)
		.child(div().flex_none().child(control))
}

fn group(title: &'static str) -> Div {
	div().flex().flex_col().child(div().pb_2().text_size(px(16.)).font_weight(FontWeight::BOLD).child(title))
}

impl Shell {
	/// A switch that sends `pref(!on)`.
	fn toggle(&self, id: &'static str, on: bool, pref: fn(bool) -> Pref, cx: &mut Context<'_, Self>) -> Switch {
		Switch::new(id).checked(on).on_click(cx.listener(move |this, checked: &bool, _, cx| this.send(Msg::Pref(pref(*checked)), cx)))
	}

	pub fn settings_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let st = self.st();
		let p = st.prefs.clone();
		let who = st.library.me.as_ref().map_or_else(|| "Spotify account".to_owned(), |me| me.display_name.clone().unwrap_or_else(|| me.id.clone()));
		let device = st.device_name.clone();
		let normalize: Vec<_> = Normalize::ALL.iter().enumerate().map(|(i, level)| {
			let level = *level;
			let label = match level {
				Normalize::Off => "Off",
				Normalize::Quiet => "Quiet",
				Normalize::Normal => "Normal",
				Normalize::Loud => "Loud",
			};
			chip(("normalize", i), label, p.audio.normalize == level, cx.listener(move |this, _, _, cx| this.send(Msg::Pref(Pref::Normalize(level)), cx)))
		}).collect();
		div()
			.id("settings")
			.size_full()
			.overflow_y_scroll()
			.child(
				div()
					.max_w(px(760.))
					.px_8()
					.pt_6()
					.pb_12()
					.flex()
					.flex_col()
					.gap_10()
					.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child("Settings"))
					.child(group("Account").child(setting(
						who,
						"Signed in on this computer",
						outline_button("sign-out", "Sign out").on_click(cx.listener(|this, _, _, cx| this.send(Msg::SignOut, cx))),
					)))
					.child(
						group("Playback")
							.child(setting("Autoplay", "Keep playing similar songs when your music ends", self.toggle("autoplay", p.playback.autoplay, Pref::Autoplay, cx)))
							.child(setting("Explicit content", "Play songs rated explicit", self.toggle("explicit", p.playback.explicit, Pref::Explicit, cx))),
					)
					.child(
						group("Audio")
							.child(setting("Normalize volume", "Play every track at the same loudness", div().flex().gap_2().children(normalize)))
							.child(setting("Mono audio", "Both speakers play the same sound", self.toggle("mono", p.audio.mono, Pref::Mono, cx))),
					)
					.child(
						group("Display")
							.child(setting("Play counts", "Show how often songs were played, where Spotify shares it", self.toggle("plays", p.display.play_counts, Pref::PlayCounts, cx)))
							.child(setting("Compact rows", "Fit more songs on screen", self.toggle("compact", p.display.compact, Pref::Compact, cx))),
					)
					.child(
						group("Sources")
							.child(setting("Lyrics", "Synced lyrics from LRCLIB", self.toggle("lyrics", p.sources.lyrics, Pref::Lyrics, cx)))
							.child(setting("Artist bios", "About text from Wikipedia", self.toggle("wikipedia", p.sources.wikipedia, Pref::Wikipedia, cx)))
							.child(setting("Spotify's web player", "Play counts, listeners and artist pages", self.toggle("web", p.sources.web_player, Pref::WebPlayer, cx))),
					)
					.child(group("About").child(setting(
						format!("turntable {}", env!("CARGO_PKG_VERSION")),
						"A lightweight Spotify player",
						div().text_size(px(14.)).text_color(theme::subdued()).child(format!("Connect name: {device}")),
					))),
			)
	}
}
