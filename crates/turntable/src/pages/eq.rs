//! The equalizer: on/off, presets, then the curve to shape by dragging its bands.

use gpui_kit::{
	Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
	component::switch::Switch, div, px,
};
use turntable_core::{Msg, audio::PRESETS};

use crate::{shell::Shell, theme, widgets::chip};

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
		let current = match (&preset, eq.enabled) {
			(_, false) => "Off".to_owned(),
			(Some(p), true) => preset_label(p),
			(None, true) => "Custom".to_owned(),
		};
		div().id("eq").size_full().overflow_y_scroll().child(
			// Centred in the main view, like Settings.
			div()
				.w_full()
				.max_w(px(760.))
				.mx_auto()
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
						.child(
							div()
								.flex()
								.flex_col()
								.gap_1()
								.child(div().text_size(px(32.)).font_weight(FontWeight::BOLD).child("Equalizer"))
								.child(div().text_size(px(14.)).text_color(theme::subdued()).child(current)),
						)
						.child(
							Switch::new("eq-on")
								.checked(eq.enabled)
								.on_click(cx.listener(|this, on: &bool, _, cx| this.send(Msg::EqEnabled(*on), cx))),
						),
				)
				.child(div().flex().flex_wrap().gap_2().children(PRESETS.iter().enumerate().map(|(i, (name, _))| {
					let name: &'static str = name;
					let on = eq.enabled && preset.as_deref() == Some(name);
					chip(
						("preset", i),
						preset_label(name),
						on,
						cx.listener(move |this, _, _, cx| {
							if !this.st().eq.enabled {
								this.send(Msg::EqEnabled(true), cx);
							}
							this.send(Msg::EqPreset(name), cx);
						}),
					)
				})))
				.child(div().rounded(px(8.)).bg(theme::raised()).pl_4().pr_6().pt_10().pb_5().child(self.eq_graph.clone())),
		)
	}
}
