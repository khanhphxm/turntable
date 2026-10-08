//! Settings: account, playback, audio, display and sources, as labelled rows with a control on
//! the right.

use gpui_kit::{
	AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
	StatefulInteractiveElement as _, Styled as _,
	component::{input::Input, switch::Switch},
	div,
	prelude::FluentBuilder as _,
	px,
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
	widgets::{Icon, chip, icon, outline_button},
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

	pub fn settings_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let query = self.settings_query.read(cx).value().trim().to_lowercase();
		let shown =
			|label: &str, help: &str| query.is_empty() || label.to_lowercase().contains(&query) || help.to_lowercase().contains(&query);
		// Each group lists the rows the search lets through; a group with none is left out.
		let groups: Vec<Div> = self
			.settings_sections(cx)
			.into_iter()
			.filter_map(|(title, rows)| {
				let rows: Vec<Div> = rows.into_iter().filter(|(l, h, _)| shown(l, h)).map(|(l, h, c)| setting(l, h, c)).collect();
				(!rows.is_empty()).then(|| group(title).children(rows))
			})
			.collect();
		let empty = groups.is_empty();
		div().id("settings").size_full().overflow_y_scroll().child(
			// Centred in the main view at Spotify's width.
			div()
				.w_full()
				.max_w(px(SETTINGS_W))
				.mx_auto()
				.px_8()
				.pt_6()
				.pb_12()
				.flex()
				.flex_col()
				.gap_8()
				.child(
					div()
						.flex()
						.items_center()
						.justify_between()
						.gap_4()
						.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child("Settings"))
						.child(self.settings_search(cx)),
				)
				.children(groups)
				.when(empty, |s| {
					s.child(div().pt_8().text_size(px(16.)).text_color(theme::subdued()).child(format!("No settings match “{query}”.")))
				}),
		)
	}

	/// Every settings row by section, in Spotify's order.
	fn settings_sections(&mut self, cx: &mut Context<'_, Self>) -> Vec<(&'static str, Vec<Row>)> {
		let st = self.st();
		let p = st.prefs.clone();
		let who = st
			.library
			.me
			.as_ref()
			.map_or_else(|| "Spotify account".to_owned(), |me| me.display_name.clone().unwrap_or_else(|| me.id.clone()));
		let device = st.device_name.clone();
		vec![
			("Account", vec![(
				who,
				"Signed in on this computer",
				outline_button("sign-out", "Sign out")
					.on_click(cx.listener(|this, _, _, cx| this.send(Msg::SignOut, cx)))
					.into_any_element(),
			)]),
			("Playback", vec![
				("Sleep timer".into(), "Pause the music after a while", self.sleep_menu(cx).into_any_element()),
				("Crossfade songs".into(), "Blend the end of one song into the next", self.crossfade_setting(cx).into_any_element()),
				(
					"Autoplay".into(),
					"Keep playing similar songs when your music ends",
					self.toggle("autoplay", p.playback.autoplay, Pref::Autoplay, cx).into_any_element(),
				),
				(
					"Explicit content".into(),
					"Play songs rated explicit",
					self.toggle("explicit", p.playback.explicit, Pref::Explicit, cx).into_any_element(),
				),
			]),
			("Audio", vec![
				("Output".into(), "Where this computer plays", self.output_menu(cx).into_any_element()),
				(
					"Normalize volume".into(),
					"Play every track at the same loudness",
					normalize_chips(p.audio.normalize, cx).into_any_element(),
				),
				(
					"Mono audio".into(),
					"Both speakers play the same sound",
					self.toggle("mono", p.audio.mono, Pref::Mono, cx).into_any_element(),
				),
			]),
			("Display", vec![
				(
					"Play counts".into(),
					"Show how often songs were played, where Spotify shares it",
					self.toggle("plays", p.display.play_counts, Pref::PlayCounts, cx).into_any_element(),
				),
				(
					"Compact rows".into(),
					"Fit more songs on screen",
					self.toggle("compact", p.display.compact, Pref::Compact, cx).into_any_element(),
				),
			]),
			("About", vec![(
				format!("{} {}", crate::menus::NAME, env!("CARGO_PKG_VERSION")),
				"A lightweight Spotify player",
				div().text_size(px(14.)).text_color(theme::subdued()).child(format!("Connect name: {device}")).into_any_element(),
			)]),
		]
	}

	/// Spotify's "Search in Settings": a magnifier that opens into a field, filtering the rows.
	fn settings_search(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let open = self.settings_search_open || !self.settings_query.read(cx).value().is_empty();
		div()
			.id("settings-search")
			.h(px(40.))
			.flex()
			.items_center()
			.gap_2()
			.px_2()
			.rounded(px(4.))
			.when(open, |s| s.w(px(240.)).bg(theme::raised()))
			.when(!open, |s| s.cursor_pointer().hover(|s| s.bg(theme::raised())))
			.on_click(cx.listener(|this, _, window, cx| {
				this.settings_search_open = true;
				this.settings_query.update(cx, |s, cx| s.focus(window, cx));
				cx.notify();
			}))
			.child(icon(Icon::Search, px(18.), theme::subdued()))
			.when(open, |s| {
				s.child(div().flex_1().text_size(px(14.)).child(Input::new(&self.settings_query).appearance(false).cleanable(true)))
			})
	}
}

/// Spotify's settings column.
const SETTINGS_W: f32 = 900.;

/// A settings row: its label, help text and control.
type Row = (String, &'static str, AnyElement);

/// "Normalize volume": Off, Quiet, Normal, Loud.
fn normalize_chips(current: Normalize, cx: &mut Context<'_, Shell>) -> Div {
	div().flex().gap_2().children(Normalize::ALL.iter().enumerate().map(|(i, level)| {
		let level = *level;
		let label = match level {
			Normalize::Off => "Off",
			Normalize::Quiet => "Quiet",
			Normalize::Normal => "Normal",
			Normalize::Loud => "Loud",
		};
		chip(("normalize", i), label, current == level, cx.listener(move |this, _, _, cx| this.send(Msg::Pref(Pref::Normalize(level)), cx)))
	}))
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
	/// Spotify's "Crossfade songs" control: while it's on, the length on a slider beside the switch.
	fn crossfade_setting(&self, cx: &mut Context<'_, Self>) -> Div {
		let seconds = self.st().prefs.playback.crossfade;
		let on = seconds > 0;
		div()
			.flex()
			.items_center()
			.gap_3()
			.when(on, |s| {
				s.child(
					div()
						.w(px(220.))
						.flex()
						.items_center()
						.gap_3()
						.text_size(px(12.))
						.text_color(theme::subdued())
						.child("0 s")
						.child(div().flex_1().child(self.crossfade.clone()))
						.child(div().w(px(36.)).child(format!("{seconds} s"))),
				)
			})
			.child(Switch::new("crossfade-on").checked(on).on_click(cx.listener(|this, on: &bool, _, cx| {
				// Spotify's default length when switched on.
				this.send(Msg::Pref(Pref::Crossfade(if *on { 5 } else { 0 })), cx);
			})))
	}
}
