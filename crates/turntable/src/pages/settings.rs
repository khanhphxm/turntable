//! Settings: account, playback, audio, display and sources, as labelled rows with a control on
//! the right.

use gpui_kit::{
	Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _,
	Styled as _, component::switch::Switch, div, prelude::FluentBuilder as _, px,
};
use turntable_core::{
	Msg,
	app::{
		rules::{Sleep, SleepChoice},
		settings::{Output, Pref},
	},
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
	#[expect(clippy::unused_self, reason = "view builders share Shell's method-call convention")]
	fn toggle(&self, id: &'static str, on: bool, pref: fn(bool) -> Pref, cx: &mut Context<'_, Self>) -> Switch {
		Switch::new(id).checked(on).on_click(cx.listener(move |this, checked: &bool, _, cx| this.send(Msg::Pref(pref(*checked)), cx)))
	}

	#[expect(clippy::too_many_lines, reason = "keep the declarative settings groups together")]
	pub fn settings_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let st = self.st();
		let p = st.prefs.clone();
		let who = st
			.library
			.me
			.as_ref()
			.map_or_else(|| "Spotify account".to_owned(), |me| me.display_name.clone().unwrap_or_else(|| me.id.clone()));
		let device = st.device_name.clone();
		let normalize: Vec<_> = Normalize::ALL
			.iter()
			.enumerate()
			.map(|(i, level)| {
				let level = *level;
				let label = match level {
					Normalize::Off => "Off",
					Normalize::Quiet => "Quiet",
					Normalize::Normal => "Normal",
					Normalize::Loud => "Loud",
				};
				chip(
					("normalize", i),
					label,
					p.audio.normalize == level,
					cx.listener(move |this, _, _, cx| this.send(Msg::Pref(Pref::Normalize(level)), cx)),
				)
			})
			.collect();
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
					who,
					"Signed in on this computer",
					outline_button("sign-out", "Sign out").on_click(cx.listener(|this, _, _, cx| this.send(Msg::SignOut, cx))),
				)))
				.child(
					group("Playback")
						.child(setting("Sleep timer", "Pause the music after a while", self.sleep_menu(cx)))
						.child(self.crossfade_setting(cx))
						.child(setting(
							"Autoplay",
							"Keep playing similar songs when your music ends",
							self.toggle("autoplay", p.playback.autoplay, Pref::Autoplay, cx),
						))
						.child(setting(
							"Explicit content",
							"Play songs rated explicit",
							self.toggle("explicit", p.playback.explicit, Pref::Explicit, cx),
						)),
				)
				.child(
					group("Audio")
						.child(setting("Output", "Where this computer plays", self.output_menu(cx)))
						.child(setting(
							"Normalize volume",
							"Play every track at the same loudness",
							div().flex().gap_2().children(normalize),
						))
						.child(setting(
							"Mono audio",
							"Both speakers play the same sound",
							self.toggle("mono", p.audio.mono, Pref::Mono, cx),
						)),
				)
				.child(
					group("Display")
						.child(setting(
							"Play counts",
							"Show how often songs were played, where Spotify shares it",
							self.toggle("plays", p.display.play_counts, Pref::PlayCounts, cx),
						))
						.child(setting(
							"Compact rows",
							"Fit more songs on screen",
							self.toggle("compact", p.display.compact, Pref::Compact, cx),
						)),
				)
				.child(
					group("Sources")
						.child(setting("Lyrics", "Synced lyrics from LRCLIB", self.toggle("lyrics", p.sources.lyrics, Pref::Lyrics, cx)))
						.child(setting(
							"Artist bios",
							"About text from Wikipedia",
							self.toggle("wikipedia", p.sources.wikipedia, Pref::Wikipedia, cx),
						))
						.child(setting(
							"Spotify's web player",
							"Play counts, listeners and artist pages",
							self.toggle("web", p.sources.web_player, Pref::WebPlayer, cx),
						)),
				)
				.child(group("About").child(setting(
					format!("turntable {}", env!("CARGO_PKG_VERSION")),
					"A lightweight Spotify player",
					div().text_size(px(14.)).text_color(theme::subdued()).child(format!("Connect name: {device}")),
				))),
		)
	}
}

/// Sleep timer choices, as Spotify offers them.
const SLEEP: [(&str, SleepChoice); 6] = [
	("Off", SleepChoice::Off),
	("15 min", SleepChoice::Minutes(15)),
	("30 min", SleepChoice::Minutes(30)),
	("45 min", SleepChoice::Minutes(45)),
	("1 hour", SleepChoice::Minutes(60)),
	("End of track", SleepChoice::EndOfTrack),
];

impl Shell {
	/// The sleep timer as a menu (the choices don't fit a row of chips beside the label).
	fn sleep_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		use gpui_kit::component::{
			button::Button,
			menu::{DropdownMenu as _, PopupMenuItem},
		};
		let current = match self.st().rules.sleep {
			None => SleepChoice::Off,
			Some(Sleep::At { minutes, .. }) => SleepChoice::Minutes(minutes),
			Some(Sleep::EndOfTrack) => SleepChoice::EndOfTrack,
		};
		let label = SLEEP.iter().find(|(_, c)| *c == current).map_or("Off", |(l, _)| *l);
		let shell = cx.entity().downgrade();
		Button::new("sleep").outline().dropdown_caret(true).label(label).dropdown_menu(move |mut menu, _, _| {
			for (label, choice) in SLEEP {
				let shell = shell.clone();
				menu = menu.item(PopupMenuItem::new(label).checked(choice == current).on_click(move |_, _, cx| {
					let _ = shell.update(cx, |this, cx| this.send(Msg::Sleep(choice), cx));
				}));
			}
			menu
		})
	}

	/// The output device picker: the system default, then every device the OS lists.
	fn output_menu(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		use gpui_kit::component::{
			button::Button,
			menu::{DropdownMenu as _, PopupMenuItem},
		};
		let sys = &self.st().sys;
		let chosen = self.st().prefs.audio.output.clone();
		let label = chosen.as_ref().map_or_else(
			|| sys.default_output.as_ref().map_or_else(|| "System default".to_owned(), |d| format!("System default ({d})")),
			|o| o.name.clone(),
		);
		let devices = sys.outputs.clone();
		let shell = cx.entity().downgrade();
		Button::new("output").outline().dropdown_caret(true).label(label).dropdown_menu(move |menu, _, _| {
			let pick = |name: String, output: Option<Output>, on: bool| {
				let shell = shell.clone();
				PopupMenuItem::new(name).checked(on).on_click(move |_, _, cx| {
					let _ = shell.update(cx, |this, cx| this.send(Msg::Pref(Pref::Output(output.clone())), cx));
				})
			};
			let mut menu = menu.item(pick("System default".to_owned(), None, chosen.is_none()));
			for d in &devices {
				let on = chosen.as_ref().is_some_and(|c| c.uid == d.uid);
				menu = menu.item(pick(d.name.clone(), Some(Output { uid: d.uid.clone(), name: d.name.clone() }), on));
			}
			menu
		})
	}
}

impl Shell {
	/// Spotify's "Crossfade songs": a switch, and while it's on the length beside a slider.
	fn crossfade_setting(&self, cx: &mut Context<'_, Self>) -> Div {
		let seconds = self.st().prefs.playback.crossfade;
		let on = seconds > 0;
		div()
			.flex()
			.flex_col()
			.child(setting(
				"Crossfade songs",
				"Blend the end of one song into the next",
				Switch::new("crossfade-on").checked(on).on_click(cx.listener(|this, on: &bool, _, cx| {
					// Spotify's default length when switched on.
					this.send(Msg::Pref(Pref::Crossfade(if *on { 5 } else { 0 })), cx);
				})),
			))
			.when(on, |s| {
				s.child(
					div()
						.flex()
						.items_center()
						.gap_3()
						.pb_2()
						.text_size(px(12.))
						.text_color(theme::subdued())
						.child("0 s")
						.child(div().flex_1().child(self.crossfade.clone()))
						.child(div().w(px(36.)).child(format!("{seconds} s"))),
				)
			})
	}
}
