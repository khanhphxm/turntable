//! The equalizer: presets, ten vertical band sliders over a ±12 dB scale, and an on/off switch.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
	component::switch::Switch, div, prelude::FluentBuilder as _, px,
};
use turntable_core::{
	Msg,
	app::{EQ_STEPS, eq_db_step},
	audio::PRESETS,
};

use crate::{shell::Shell, theme, widgets::chip};

const BANDS: [&str; 10] = ["32", "64", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"];
const RANGE_DB: f32 = 12.;

pub fn db_to_value(db: f32) -> f32 {
	(db / RANGE_DB + 1.) / 2.
}

/// A slider position (0..=1) as the core's band step.
pub fn value_to_step(v: f32) -> u8 {
	eq_db_step((v * 2. - 1.) * RANGE_DB).min(EQ_STEPS)
}

/// "bass_boost" → "Bass boost".
fn preset_label(name: &str) -> String {
	let mut s = name.replace('_', " ");
	if let Some(first) = s.get(..1) {
		let upper = first.to_uppercase();
		s.replace_range(..1, &upper);
	}
	s
}

impl Shell {
	pub fn eq_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let eq = self.st().eq;
		let preset = self.st().eq_preset.clone();
		div()
			.id("eq")
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
					.gap_6()
					.child(
						div()
							.flex()
							.items_center()
							.justify_between()
							.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child("Equalizer"))
							.child(Switch::new("eq-on").checked(eq.enabled).on_click(cx.listener(|this, on: &bool, _, cx| this.send(Msg::EqEnabled(*on), cx)))),
					)
					.child(div().flex().flex_wrap().gap_2().children(PRESETS.iter().enumerate().map(|(i, (name, _))| {
						let name: &'static str = name;
						chip(("preset", i), preset_label(name), preset.as_deref() == Some(name), cx.listener(move |this, _, _, cx| this.send(Msg::EqPreset(name), cx)))
					})))
					.child(
						div()
							.h(px(280.))
							.rounded(px(8.))
							.bg(theme::raised())
							.px_6()
							.py_5()
							.flex()
							.gap_4()
							.when(!eq.enabled, |s| s.opacity(0.4))
							// The dB scale on the left, its labels level with the top, middle and bottom.
							.child(
								div()
									.w(px(44.))
									.flex()
									.flex_col()
									.justify_between()
									.pb(px(24.))
									.text_size(px(12.))
									.text_color(theme::subdued())
									.child("+12 dB")
									.child("0 dB")
									.child("−12 dB"),
							)
							.child(div().flex_1().flex().justify_between().children(self.eq.iter().zip(BANDS).map(|(slider, label)| {
								div()
									.flex()
									.flex_col()
									.items_center()
									.gap_2()
									.child(div().flex_1().child(slider.clone()))
									.child(div().h(px(16.)).text_size(px(12.)).text_color(theme::subdued()).child(label))
							}))),
					)
					.child(div().text_size(px(14.)).text_color(theme::subdued()).child("Applied on this computer, to everything turntable plays.")),
			)
	}
}
