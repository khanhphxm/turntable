//! Colours, sizes and type: one dark theme modelled on Spotify's desktop app.

use gpui_kit::{Hsla, Pixels, Rgba, px, rgb, rgba};

/// The window behind the panels.
pub fn base() -> Rgba {
	rgb(0x0000_0000)
}
/// A panel (library, main view, right-hand panel).
pub fn panel() -> Rgba {
	rgb(0x0012_1212)
}
/// A control's resting fill (search field, round buttons).
pub fn raised() -> Rgba {
	rgb(0x001f_1f1f)
}
/// The same, hovered.
pub fn raised_hover() -> Rgba {
	rgb(0x002a_2a2a)
}
/// A row or card under the pointer.
pub fn hover() -> Rgba {
	rgba(0xffff_ff12)
}
/// A selected row.
pub fn selected() -> Rgba {
	rgba(0xffff_ff26)
}
pub fn text() -> Rgba {
	rgb(0x00ff_ffff)
}
pub fn subdued() -> Rgba {
	rgb(0x00b3_b3b3)
}
pub fn faint() -> Rgba {
	rgb(0x007c_7c7c)
}
/// Spotify's green: playing, saved, active toggles.
pub fn accent() -> Rgba {
	rgb(0x001e_d760)
}
/// A seek or volume bar's track.
pub fn track() -> Rgba {
	rgba(0xffff_ff4d)
}
pub fn divider() -> Rgba {
	rgba(0xffff_ff1a)
}

/// The gap between panels and around the window's edge.
pub const GAP: Pixels = px(8.);
/// Panel corner radius.
pub const RADIUS: Pixels = px(8.);
pub const BAR_H: Pixels = px(72.);
pub const TOP_H: Pixels = px(64.);
pub const SIDEBAR_W: Pixels = px(320.);
pub const SIDEBAR_COLLAPSED_W: Pixels = px(72.);
/// Your Library widened by its expand button.
pub const SIDEBAR_WIDE_W: Pixels = px(584.);
/// Spotify's right panel at its default width.
pub const RIGHT_W: Pixels = px(280.);
/// The right panel folded to a strip with a chevron.
pub const RIGHT_FOLDED_W: Pixels = px(28.);
pub const ROW_H: Pixels = px(56.);

/// `c` at `alpha` (0..=1).
pub fn alpha(c: Rgba, alpha: f32) -> Hsla {
	let mut h: Hsla = c.into();
	h.a *= alpha;
	h
}

/// Card width on shelves (cover + 12 px padding each side).
pub const CARD_W: f32 = 188.;

pub fn accent_hover() -> Rgba {
	rgb(0x003b_e477)
}

/// [`raised`] as a number, for artwork placeholders.
pub const RAISED: u32 = 0x001f_1f1f;

/// "#1db954" or "1db954" → 0x1db954.
pub fn parse_hex(s: &str) -> Option<u32> {
	let h = s.trim_start_matches('#');
	if h.len() == 6 { u32::from_str_radix(h, 16).ok() } else { None }
}
