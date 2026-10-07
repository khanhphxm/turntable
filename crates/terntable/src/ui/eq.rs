//! The equalizer panel: a graph of ten bands, each a track with a thumb at its gain, joined by a
//! curve; the on/off switch; and a menu of presets. It shares the right-hand slot with the queue.
//!
//! Tern reports clicks, not pointer positions or drags. A band's track is therefore a stack of
//! invisible cells, one per 2 dB step, and a click on one sets that step; hovering a cell shows
//! where the thumb would go. The thumbs and the curve show the gains to the nearest dB, because
//! presets use odd values too.

use tern_sdk::{
	Session,
	ui::{self, Node, html},
};

use super::{Tap, div, icon, picture, text};
use crate::{
	app::{EQ_STEPS, Layout, Msg, Panel, State, eq_db_step, eq_step_db},
	audio::BANDS_HZ,
	ui::icons::{Glyph, Icons},
};

/// Presets in the order shown, with their labels.
const PRESETS: [(&str, &str); 7] = [
	("flat", "Flat"),
	("bass_boost", "Bass boost"),
	("bass_cut", "Bass cut"),
	("vocal", "Vocal"),
	("treble_boost", "Treble boost"),
	("loudness", "Loudness"),
	("rock", "Rock"),
];

/// Where the preset button sits in the view: the menu hangs from it.
const PRESET_NODE: &str = "main.app.eq.presets.select";

/// The gain limit either way, in dB. The sheet's `.tt-g0` … `.tt-g24` are the 25 whole-dB heights
/// a thumb can stand at.
const RANGE_DB: f32 = 12.0;

/// The size the response curve is drawn for, in px. Tern stretches the picture over the plot, so this
/// only sets how thick the line is.
const CURVE_PX: (f64, f64) = (240.0, 360.0);

pub fn panel(state: &State, icons: &Icons) -> Node<Msg> {
	let on = state.eq.enabled;
	let switch = html::el("button")
		.class(if on { "tt-switch tt-on" } else { "tt-switch" })
		.title(if on { "Turn the equalizer off" } else { "Turn the equalizer on" })
		.aria("Equalizer")
		.child(html::span("").class("tt-knob"))
		.on_click(Msg::EqEnabled(!on));
	let mut panel = div("tt-panel tt-eq").key("eq").child(div("tt-eq-head").key("head").child(text("tt-h2", "Equalizer")).child(switch));
	if !state.has_audio {
		panel = panel.child(note("The equalizer applies when the player's audio output is running."));
	}
	panel.child(preset_button(state, icons)).child(graph(state)).into()
}

fn note(words: &str) -> Node<Msg> {
	div("tt-note tt-eq-note").key("note").child(html::span("i").class("tt-eq-i")).child(text("tt-eq-note-text", words.to_owned())).into()
}

/// The preset the bands stand at: the one picked, `flat` while every band is at 0 dB, none once
/// they were set by hand.
fn current_preset(state: &State) -> Option<&'static str> {
	let picked = state.eq_preset.as_deref().and_then(|id| PRESETS.iter().find(|(preset, _)| *preset == id));
	match picked {
		Some((id, _)) => Some(id),
		None => state.eq.gains_db.iter().all(|gain| *gain == 0.0).then_some("flat"),
	}
}

/// The preset button: its caption, the current preset ("Custom" once the bands were set by hand) and
/// a chevron that turns over while the menu is open.
fn preset_button(state: &State, icons: &Icons) -> Node<Msg> {
	let current = current_preset(state);
	let label = PRESETS.iter().find(|(id, _)| Some(*id) == current).map_or("Custom", |&(_, label)| label);
	let select = html::el("button")
		.class(if state.eq_menu_open() { "tt-select tt-open" } else { "tt-select" })
		.key("select")
		.aria("Preset")
		.child(html::span("Preset").class("tt-select-cap"))
		.child(html::span(label).class("tt-select-val"))
		.child(div("tt-select-chev").child(icon(icons, Glyph::Down, 16.0, Tap::Msg(Msg::EqMenu, None))))
		.on_click(Msg::EqMenu);
	div("tt-eq-presets").key("presets").child(select).into()
}

/// The preset menu, under the preset button: an overlay, as Your Library's menu is, over a scrim that
/// closes it on a click anywhere else (Tern doesn't say when a click lands elsewhere).
pub fn menu(state: &State, icons: &Icons) -> Vec<Node<Msg>> {
	if !state.eq_menu_open() {
		return Vec::new();
	}
	let current = current_preset(state);
	let mut list = div("tt-menu").key("list");
	for (id, label) in PRESETS {
		let pick = Msg::EqPreset(id);
		let mut item = html::el("button")
			.class(if current == Some(id) { "tt-menu-item tt-on" } else { "tt-menu-item" })
			.key(id)
			.child(html::span(label))
			.on_click(pick.clone());
		if current == Some(id) {
			item = item.child(icon(icons, Glyph::Check, 16.0, Tap::Msg(pick, None)));
		}
		list = list.child(item);
	}
	// As wide as the preset button: the side column's, or on the compact layout, where the panel is the
	// page, the pane's less its margins (see the sheet).
	let role = if state.layout() == Layout::Compact { "tt.eqmenu-page" } else { "tt.eqmenu" };
	vec![
		div("tt-eq-scrim").key("eq-scrim").on_click(Msg::EqMenu).into(),
		ui::overlay()
			.key("eq-menu")
			.role(role)
			.anchor(ui::Anchor::Node { id: PRESET_NODE.to_owned(), above: false })
			.size(ui::OverlaySize::Sm)
			.child(list)
			.into(),
	]
}

/// The graph: the dB scale beside the plot of ten bands, and the frequencies under it.
fn graph(state: &State) -> Node<Msg> {
	let eq = state.eq.sanitized();
	// Guides every 6 dB; the one at 0 dB is the reference.
	let mut field = div("tt-eq-field").key("field");
	for at in [0, 25, 50, 75, 100] {
		field = field.child(div(&format!("tt-eq-line tt-at{at}")).key(at));
	}
	if let Some((_, blob)) = &state.eq_curve {
		let curve = ui::image().blob(blob.clone()).alt("").w(CURVE_PX.0).h(CURVE_PX.1).role("tt.eqcurve");
		field = field.child(div("tt-eq-curve").key("curve").child(picture(curve, Tap::NONE)));
	}
	let mut bands = div("tt-eq-bands").key("bands");
	let mut labels = div("tt-eq-hz").key("hz").child(text("tt-eq-unit", "Hz"));
	for (index, (hz, gain)) in BANDS_HZ.iter().zip(eq.gains_db).enumerate() {
		bands = bands.child(band(index, *hz, gain));
		labels = labels.child(text("tt-num", band_label(*hz)).key(format!("l{index}")));
	}
	let axis = div("tt-eq-axis")
		.key("axis")
		.child(text("tt-eq-unit", "dB"))
		.child(text("tt-eq-tick tt-at0", db_label(RANGE_DB)))
		.child(text("tt-eq-tick tt-at50", db_label(0.0)))
		.child(text("tt-eq-tick tt-at100", db_label(-RANGE_DB)));
	div(if eq.enabled { "tt-eq-graph" } else { "tt-eq-graph tt-off" })
		.key("graph")
		.child(div("tt-eq-plot").key("plot").child(axis).child(field.child(bands)))
		.child(labels)
		.into()
}

/// One band: its track with the thumb at the gain, the gain above the plot unless it is 0 dB, and
/// over them the invisible cells that take the clicks.
fn band(index: usize, hz: f32, gain: f32) -> Node<Msg> {
	let level = gain.round();
	let nearest = eq_db_step(gain);
	let mut cells = div("tt-eq-cells").key("cells");
	// From +12 dB down. The end cells are half cells, so every step is centred on its own height.
	for step in (0..=EQ_STEPS).rev() {
		let mut class = String::from("tt-eq-cell");
		if step == EQ_STEPS {
			class.push_str(" tt-first");
		} else if step == 0 {
			class.push_str(" tt-last");
		}
		if step == nearest {
			class.push_str(" tt-cur");
		}
		cells = cells.child(
			html::span("").class(class).key(u32::from(step)).attr("data-db", db_label(eq_step_db(step))).on_click(Msg::EqBand(index, step)),
		);
	}
	// `level` is a whole number, so it prints without a fraction: 0 dB is `tt-g12`.
	let mut column = div(&format!("tt-band tt-g{}", level + RANGE_DB)).key(format!("b{index}")).title(format!(
		"{} \u{b7} {} dB",
		band_hz(hz),
		db_label(level)
	));
	if level != 0.0 {
		column = column.child(text("tt-eq-val tt-num", db_label(level)));
	}
	column
		.child(html::span("").class("tt-eq-rail"))
		.child(html::span("").class("tt-eq-act"))
		.child(html::span("").class("tt-eq-thumb"))
		.child(cells)
		.into()
}

/// `0`, `+4`, `−6` (a true minus sign, so the column of labels lines up).
fn db_label(db: f32) -> String {
	if db > 0.0 {
		format!("+{db}")
	} else if db < 0.0 {
		format!("\u{2212}{}", -db)
	} else {
		"0".to_owned()
	}
}

/// `32`, `63`, …, `1k`, `16k`.
fn band_label(hz: f32) -> String {
	if hz >= 1000.0 { format!("{}k", (hz / 1000.0).round()) } else { hz.round().to_string() }
}

/// `32 Hz`, `1 kHz`.
fn band_hz(hz: f32) -> String {
	if hz >= 1000.0 { format!("{} kHz", (hz / 1000.0).round()) } else { format!("{} Hz", hz.round()) }
}

/// The response curve through the thumbs as an SVG: a line over a fill that fades towards 0 dB. It
/// is the monotone cubic through the ten points, so it never overshoots a thumb, and runs from the
/// first band's thumb to the last's (a point sits on the middle of its band's column).
fn curve_svg(gains: &[f32; 10]) -> String {
	let (w, h) = CURVE_PX;
	let column = w / 10.0;
	let row = h / f64::from(2.0 * RANGE_DB);
	let ys: Vec<f64> = gains.iter().map(|gain| f64::from(RANGE_DB - gain.round()) * row).collect();
	let slopes: Vec<f64> = ys.iter().zip(ys.iter().skip(1)).map(|(a, b)| (b - a) / column).collect();
	// Tangents: flat at both ends and at every extreme, else the harmonic mean of the slopes either side.
	let inner = slopes.iter().zip(slopes.iter().skip(1)).map(|(a, b)| if a * b > 0.0 { 2.0 * a * b / (a + b) } else { 0.0 });
	let tangents: Vec<f64> = std::iter::once(0.0).chain(inner).chain(std::iter::once(0.0)).collect();
	let knots: Vec<(f64, f64, f64)> =
		ys.iter().zip(&tangents).enumerate().map(|(i, (y, tangent))| (column * (i as f64 + 0.5), *y, *tangent)).collect();
	let [(_, first, _), ..] = knots.as_slice() else {
		return String::new();
	};
	let mut path = vec![format!("M{} {}", num(column / 2.0), num(*first))];
	path.extend(knots.windows(2).filter_map(|pair| {
		let [(x0, y0, t0), (x1, y1, t1)] = pair else {
			return None;
		};
		Some(format!(
			"C{} {} {} {} {} {}",
			num(x0 + column / 3.0),
			num(y0 + t0 * column / 3.0),
			num(x1 - column / 3.0),
			num(y1 - t1 * column / 3.0),
			num(*x1),
			num(*y1),
		))
	}));
	let (start, end) = (num(column / 2.0), num(w - column / 2.0));
	let d = path.join(" ");
	let zero = h / 2.0;
	format!(
		r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" preserveAspectRatio="none"><defs><linearGradient id="f" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="0" y2="{h}"><stop offset="0" stop-color="currentColor" stop-opacity=".4"/><stop offset=".5" stop-color="currentColor" stop-opacity="0"/><stop offset="1" stop-color="currentColor" stop-opacity=".4"/></linearGradient></defs><path d="{d} L{end} {zero} L{start} {zero} Z" fill="url(#f)"/><path d="{d}" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" vector-effect="non-scaling-stroke"/></svg>"#
	)
}

/// A coordinate to two decimals, without trailing zeros.
fn num(v: f64) -> String {
	format!("{v:.2}").trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The gains whose response curve has no picture yet: the equalizer is on screen and the picture
/// Tern has, if any, was drawn for other gains.
fn undrawn(state: &State) -> Option<[f32; 10]> {
	if state.side_panel() != Some(Panel::Eq) || state.layout() == Layout::Strip {
		return None;
	}
	let gains = state.eq.sanitized().gains_db;
	state.eq_curve.as_ref().is_none_or(|(drawn, _)| *drawn != gains).then_some(gains)
}

/// Sends Tern the picture of the response curve when the equalizer is on screen and the gains
/// changed since the last one. The view names it by blob id, so this runs before each render.
pub fn send_curve<M>(session: &mut Session<M>, state: &mut State) -> Result<(), tern_sdk::Error> {
	if let Some(gains) = undrawn(state) {
		let blob = session.blob(curve_svg(&gains).into_bytes(), Some("image/svg+xml"))?;
		state.eq_curve = Some((gains, blob));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn with_gains(gains: [f32; 10]) -> State {
		let mut state = State { panel: Some(Panel::Eq), ..State::default() };
		state.eq.gains_db = gains;
		state
	}

	#[test]
	fn slider_steps_round_trip_through_decibels() {
		for step in 0..=EQ_STEPS {
			assert_eq!(eq_db_step(eq_step_db(step)), step);
		}
		assert_eq!(eq_step_db(EQ_STEPS / 2), 0.0);
		assert_eq!(eq_db_step(99.0), EQ_STEPS);
	}

	#[test]
	fn the_curve_never_overshoots_a_thumb() {
		// Parse the path back: each Bézier's control heights must stay between its end heights.
		let svg = curve_svg(&[0.0, 12.0, 12.0, -12.0, 4.0, 5.0, 4.0, 0.0, 12.0, 0.0]);
		let d = svg.split(" d=\"").nth(1).and_then(|s| s.split('"').next()).unwrap();
		let mut start: f64 = d.split(' ').nth(1).unwrap().parse().unwrap();
		let mut segments = 0;
		for segment in d.split(" C").skip(1) {
			let n: Vec<f64> = segment.split(' ').take(6).map(|v| v.trim_start_matches('L').parse().unwrap()).collect();
			let (c1, c2, end) = (n[1], n[3], n[5]);
			let (lo, hi) = (start.min(end) - 1e-6, start.max(end) + 1e-6);
			assert!((lo..=hi).contains(&c1) && (lo..=hi).contains(&c2), "{start} -> {end}: controls {c1}, {c2}");
			start = end;
			segments += 1;
		}
		assert_eq!(segments, 9);
	}

	#[test]
	fn the_curve_is_drawn_once_per_set_of_gains_while_the_panel_shows() {
		let mut state = with_gains([0.0; 10]);
		assert_eq!(undrawn(&state), Some([0.0; 10]));
		state.eq_curve = Some(([0.0; 10], "blob".into()));
		assert_eq!(undrawn(&state), None, "already drawn");
		state.eq.gains_db[3] = 2.0;
		assert_eq!(undrawn(&state), Some(state.eq.gains_db), "changed");
		state.panel = Some(Panel::Queue);
		assert_eq!(undrawn(&state), None, "not on screen");
	}
}
