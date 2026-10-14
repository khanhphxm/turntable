//! Bundled assets: Lucide icons (only those named below are embedded), the app icon and Turntable's
//! filled glyphs, with GPUI Kit's component icons behind them. The transport glyphs are original
//! triangles and rectangles, not copied or traced from Spotify artwork, and use Turntable's MIT license.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

/// One list names every icon: Lucide's (from GPUI Kit's bundled set, only these embedded) and
/// Turntable's filled glyphs, including the original geometric transport icons.
macro_rules! icons {
	(lucide { $($lv:ident => $lucide:ident),* $(,)? } own { $($ov:ident => $file:literal),* $(,)? }) => {
		/// The bundled icons.
		#[derive(Debug, Clone, Copy, PartialEq, Eq)]
		pub enum Icon { $($lv,)* $($ov,)* }

		impl Icon {
			pub fn path(self) -> SharedString {
				match self {
					$(Self::$lv => gpui_kit::assets::IconName::$lucide.path(),)*
					$(Self::$ov => SharedString::new_static(concat!("icons/", $file, ".svg")),)*
				}
			}
		}

		gpui_kit::assets::icon_assets!(Lucide, [$($lucide),*]);

		const OWN: &[(&str, &[u8])] = &[
			("icons/app.svg", include_bytes!("../assets/icons/app.svg")),
			$((concat!("icons/", $file, ".svg"), include_bytes!(concat!("../assets/icons/", $file, ".svg")))),*
		];
	};
}

icons! {
	lucide {
		Back => ChevronLeft,
		Check => Check,
		Clock => Clock3,
		Close => X,
		Collapse => Minimize2,
		Computer => Laptop,
		Copy => Copy,
		Device => MonitorSpeaker,
		Disc => Disc3,
		Ellipsis => Ellipsis,
		Expand => Maximize2,
		Folder => Folder,
		Forward => ChevronRight,
		Fullscreen => Maximize,
		Gear => Settings,
		Home => House,
		MenuAddTo => ListPlus,
		MenuAlbum => Disc3,
		MenuArtist => UserRound,
		MenuChevron => ChevronRight,
		MenuCollab => UsersRound,
		MenuDelete => CircleMinus,
		MenuEdit => Pencil,
		MenuExclude => CircleX,
		MenuFolder => Folder,
		MenuLink => Link,
		MenuNext => ListStart,
		MenuNewPlaylist => ListMusic,
		MenuPin => Pin,
		MenuPlus => Plus,
		MenuPublic => Globe,
		MenuQueue => ListEnd,
		MenuRemove => CircleCheck,
		MenuShare => Share,
		Mic => Mic,
		Miniplayer => PictureInPicture2,
		Music => Music,
		Mute => VolumeX,
		Offline => WifiOff,
		PanelClose => PanelRightClose,
		Pencil => Pencil,
		Phone => Smartphone,
		Pin => Pin,
		Plus => Plus,
		Queue => ListMusic,
		Repeat => Repeat,
		RepeatOne => Repeat1,
		Save => CirclePlus,
		Search => Search,
		Share => Share,
		Shuffle => Shuffle,
		SidebarClose => PanelLeftClose,
		SidebarOpen => PanelLeftOpen,
		Sliders => SlidersHorizontal,
		Speaker => Speaker,
		Tv => Tv,
		ViewCompact => Rows3,
		ViewGrid => LayoutGrid,
		ViewLarge => LayoutList,
		ViewList => List,
		Volume => Volume2,
	}
	own {
		Heart => "heart",
		Next => "next",
		Pause => "pause",
		Play => "play",
		Prev => "prev",
	}
}

pub struct Assets;

impl AssetSource for Assets {
	fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
		if let Some((_, bytes)) = OWN.iter().find(|(p, _)| *p == path) {
			return Ok(Some(Cow::Borrowed(bytes)));
		}
		if let Some(bytes) = Lucide.load(path)? {
			return Ok(Some(bytes));
		}
		gpui_kit::assets::Assets.load(path)
	}

	fn list(&self, path: &str) -> Result<Vec<SharedString>> {
		let mut all: Vec<SharedString> = OWN.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| (*p).into()).collect();
		all.extend(Lucide.list(path)?);
		all.extend(gpui_kit::assets::Assets.list(path)?);
		all.sort();
		all.dedup();
		Ok(all)
	}
}
