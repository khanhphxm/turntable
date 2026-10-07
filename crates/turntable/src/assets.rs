//! Bundled assets: turntable's own icons (`assets/icons/<name>.svg`, one 24 px grid, 2 px round
//! strokes in `currentColor`), with GPUI Kit's icon set behind them for the components that ask
//! for theirs.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

/// One list names every icon: the enum, its asset path and the embedded bytes stay in step.
macro_rules! icons {
	($($variant:ident => $file:literal),* $(,)?) => {
		/// The bundled icons.
		#[derive(Debug, Clone, Copy, PartialEq, Eq)]
		pub enum Icon { $($variant),* }

		impl Icon {
			pub const fn path(self) -> &'static str {
				match self { $(Self::$variant => concat!("icons/", $file, ".svg")),* }
			}
		}

		const OWN: &[(&str, &[u8])] = &[$((concat!("icons/", $file, ".svg"), include_bytes!(concat!("../assets/icons/", $file, ".svg")))),*];
	};
}

icons! {
	App => "app",
	Back => "back",
	Bell => "bell",
	Check => "check",
	Clock => "clock",
	Close => "close",
	Computer => "computer",
	Device => "device",
	Down => "down",
	Ellipsis => "ellipsis",
	External => "external",
	Forward => "forward",
	Gear => "gear",
	Globe => "globe",
	Heart => "heart",
	Home => "home",
	Library => "library",
	Lock => "lock",
	Mic => "mic",
	Moon => "moon",
	Mute => "mute",
	Next => "next",
	NowPlaying => "now-playing",
	Pause => "pause",
	Pencil => "pencil",
	Phone => "phone",
	Play => "play",
	Plus => "plus",
	Prev => "prev",
	Queue => "queue",
	Repeat => "repeat",
	RepeatOne => "repeat-one",
	Save => "save",
	Search => "search",
	Shuffle => "shuffle",
	SidebarClose => "sidebar-close",
	SidebarOpen => "sidebar-open",
	Sliders => "sliders",
	Speaker => "speaker",
	Tv => "tv",
	User => "user",
	ViewCompact => "view-compact",
	ViewGrid => "view-grid",
	ViewLarge => "view-large",
	ViewList => "view-list",
	Volume => "volume",
}

pub struct Assets;

impl AssetSource for Assets {
	fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
		if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
			return Ok(Some(Cow::Borrowed(bytes)));
		}
		gpui_kit::assets::Assets.load(path)
	}

	fn list(&self, path: &str) -> Result<Vec<SharedString>> {
		let mut all: Vec<SharedString> = OWN.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| (*p).into()).collect();
		all.extend(gpui_kit::assets::Assets.list(path)?);
		Ok(all)
	}
}
