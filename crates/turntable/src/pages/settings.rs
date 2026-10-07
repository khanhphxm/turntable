//! Settings: account, playback, display and about, as labelled rows with a control on the right.

use gpui_kit::{
	Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _,
	Styled as _, component::switch::Switch, div, px,
};

use crate::{
	shell::{Prefs, Shell},
	theme,
	widgets::{chip, outline_button},
};

/// A settings row: label and help text on the left, the control on the right.
fn setting(label: &'static str, help: &'static str, control: impl IntoElement) -> Div {
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
				.child(div().text_size(px(16.)).child(label))
				.child(div().text_size(px(14.)).text_color(theme::subdued()).child(help)),
		)
		.child(div().flex_none().child(control))
}

fn group(title: &'static str) -> Div {
	div().flex().flex_col().child(div().pb_2().text_size(px(16.)).font_weight(FontWeight::BOLD).child(title))
}

impl Shell {
	fn toggle(&self, id: &'static str, on: bool, flip: fn(&mut Prefs), cx: &mut Context<'_, Self>) -> Switch {
		Switch::new(id).checked(on).on_click(cx.listener(move |this, _: &bool, _, cx| {
			flip(&mut this.prefs);
			cx.notify();
		}))
	}

	pub fn settings_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let quality = self.prefs.quality;
		let quality_chip = |i: usize, label: &'static str, cx: &mut Context<'_, Self>| {
			chip(
				("quality", i),
				label,
				quality == i,
				cx.listener(move |this, _, _, cx| {
					this.prefs.quality = i;
					cx.notify();
				}),
			)
		};
		let p = &self.prefs;
		let (normalize, crossfade, autoplay, lyrics_bar) = (p.normalize, p.crossfade, p.autoplay, p.lyrics_bar);
		div().id("settings").size_full().overflow_y_scroll().child(
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
					"khanh",
					"Spotify Premium • signed in on this computer",
					outline_button("sign-out", "Sign out"),
				)))
				.child(
					group("Playback")
						.child(setting(
							"Streaming quality",
							"Higher quality uses more data",
							div()
								.flex()
								.gap_2()
								.child(quality_chip(0, "Low", cx))
								.child(quality_chip(1, "Normal", cx))
								.child(quality_chip(2, "Very high", cx)),
						))
						.child(setting(
							"Normalize volume",
							"Play every track at the same loudness",
							self.toggle("normalize", normalize, |p| p.normalize = !p.normalize, cx),
						))
						.child(setting(
							"Crossfade songs",
							"Blend the end of one song into the next",
							self.toggle("crossfade", crossfade, |p| p.crossfade = !p.crossfade, cx),
						))
						.child(setting(
							"Autoplay",
							"Keep playing similar songs when your music ends",
							self.toggle("autoplay", autoplay, |p| p.autoplay = !p.autoplay, cx),
						)),
				)
				.child(group("Display").child(setting(
					"Lyrics button",
					"Show the lyrics button in the player bar",
					self.toggle("lyrics-bar", lyrics_bar, |p| p.lyrics_bar = !p.lyrics_bar, cx),
				)))
				.child(
					group("About").child(setting(
						"turntable",
						"A lightweight Spotify player",
						div()
							.text_size(px(14.))
							.text_color(theme::subdued())
							.child(SharedString::from(concat!("Version ", env!("CARGO_PKG_VERSION")))),
					)),
				),
		)
	}
}
