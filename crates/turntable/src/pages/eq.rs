//! The equalizer: presets, ten vertical band sliders over a ±12 dB scale, and an on/off switch.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
	component::switch::Switch, div, prelude::FluentBuilder as _, px,
};

use crate::{shell::Shell, theme, widgets::chip};

const BANDS: [&str; 10] = ["32", "64", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"];
/// (name, gain per band in dB).
pub const PRESETS: [(&str, [f32; 10]); 6] = [
	("Flat", [0.; 10]),
	("Bass Booster", [6., 5., 4., 2.5, 1., 0., 0., 0., 0., 0.]),
	("Treble Booster", [0., 0., 0., 0., 0., 1., 2.5, 4., 5., 6.]),
	("Vocal", [-2., -1.5, -1., 1., 3., 3.5, 3., 1.5, 0., -1.]),
	("Acoustic", [4., 3.5, 2.5, 1., 1.5, 1.5, 2.5, 3., 2.5, 1.5]),
	("Electronic", [4.5, 4., 1., 0., -2., 1.5, 1., 1.5, 4., 5.]),
];
const RANGE_DB: f32 = 12.;

pub fn db_to_value(db: f32) -> f32 {
	(db / RANGE_DB + 1.) / 2.
}

impl Shell {
	pub fn apply_preset(&mut self, name: &'static str, cx: &mut Context<'_, Self>) {
		if let Some((_, gains)) = PRESETS.iter().find(|(n, _)| *n == name) {
			for (slider, db) in self.eq.iter().zip(gains) {
				slider.update(cx, |s, cx| s.set(db_to_value(*db), cx));
			}
			self.eq_preset = Some(name);
			cx.notify();
		}
	}

	pub fn eq_page(&mut self, cx: &mut Context<'_, Self>) -> impl IntoElement {
		let on = self.eq_on;
		let preset = self.eq_preset;
		div().id("eq").size_full().overflow_y_scroll().child(
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
						.child(Switch::new("eq-on").checked(on).on_click(cx.listener(|this, _: &bool, _, cx| {
							this.eq_on = !this.eq_on;
							cx.notify();
						}))),
				)
				.child(div().flex().flex_wrap().gap_2().children(PRESETS.iter().enumerate().map(|(i, (name, _))| {
					let name = *name;
					chip(("preset", i), name, preset == Some(name), cx.listener(move |this, _, _, cx| this.apply_preset(name, cx)))
				})))
				.child(
					div()
						.relative()
						.h(px(280.))
						.rounded(px(8.))
						.bg(theme::raised())
						.px_6()
						.py_5()
						.flex()
						.gap_4()
						.when(!on, |s| s.opacity(0.4))
						// The dB scale on the left.
						.child(
							div()
								.w(px(40.))
								.flex()
								.flex_col()
								.justify_between()
								.pb(px(24.))
								.text_size(px(12.))
								.text_color(theme::subdued())
								.child("+12 dB")
								.child("0 dB")
								.child("-12 dB"),
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
				.child(
					div().text_size(px(14.)).text_color(theme::subdued()).child("Applied on this computer, to everything turntable plays."),
				),
		)
	}
}
