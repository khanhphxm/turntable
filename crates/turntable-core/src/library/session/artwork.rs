//! Playlist artwork from native Playlist4 attributes, with album mosaics only when
//! Spotify omitted a picture. No Web API or anonymous web-player lookup is involved.

use std::fmt::Write as _;

use librespot_core::{FileId, Session};
use librespot_protocol::{
	extension_kind::ExtensionKind,
	metadata,
	playlist4_external::{ListAttributes, SelectedListContent},
};
use smallvec::SmallVec;

use super::{metadata as fetch_metadata, playlist};
use crate::{
	library::PAGE,
	spotify::{self, Image, Playlist, Track},
};

fn image(url: String, side: Option<u32>) -> Image {
	Image { url, width: side, height: side }
}

fn hex_id(value: &str) -> Option<FileId> {
	if value.len() != 40 {
		return None;
	}
	let mut raw = [0; 20];
	for (out, pair) in raw.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
		*out = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
	}
	Some(FileId(raw))
}

fn url(value: &str) -> Option<String> {
	if value.starts_with("https://") {
		return Some(value.to_owned());
	}
	if let Some(ids) = value.strip_prefix("spotify:mosaic:") {
		let mut result = String::with_capacity(185);
		result.push_str("https://mosaic.scdn.co/640/");
		result.extend(ids.chars().filter(|character| *character != ':'));
		let ids = result.strip_prefix("https://mosaic.scdn.co/640/")?;
		return (ids.len() == 160 && ids.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(result);
	}
	let id = hex_id(value.strip_prefix("spotify:image:").unwrap_or(value))?;
	Some(format!("https://i.scdn.co/image/{id}"))
}

pub(super) fn images(attributes: &ListAttributes) -> Vec<Image> {
	let mut out: Vec<_> = attributes
		.picture_size
		.iter()
		.filter_map(|picture| {
			let side = match picture.target_name() {
				"small" => Some(64),
				"default" => Some(300),
				"large" => Some(640),
				_ => None,
			};
			Some(image(url(picture.url())?, side))
		})
		.collect();
	if !out.is_empty() {
		return out;
	}
	if let Some(url) = std::str::from_utf8(attributes.picture()).ok().and_then(url) {
		out.push(image(url, None));
	} else if attributes.picture().len() == 20 {
		out.push(image(format!("https://i.scdn.co/image/{}", FileId::from_raw(attributes.picture())), None));
	}
	if out.is_empty() {
		for attribute in &attributes.format_attributes {
			if matches!(attribute.key(), "image" | "image_url" | "image_uri" | "picture" | "picture_url" | "cover_image_url")
				&& let Some(url) = url(attribute.value())
			{
				out.push(image(url, None));
				break;
			}
		}
	}
	out
}

#[derive(Default)]
struct Mosaic(SmallVec<[FileId; 4]>);

impl Mosaic {
	fn add(&mut self, id: FileId) {
		if self.0.len() < 4 && !self.0.contains(&id) {
			self.0.push(id);
		}
	}

	fn rows(&mut self, tracks: &[Track]) {
		for track in tracks {
			if let Some(cover) = track.album.as_ref().and_then(|album| spotify::pick_image(&album.images, 640))
				&& let Some(id) = cover.url.strip_prefix("https://i.scdn.co/image/").and_then(hex_id)
			{
				self.add(id);
			}
			if self.0.len() == 4 {
				break;
			}
		}
	}

	fn finish(self) -> Vec<Image> {
		let Some(first) = self.0.first() else { return Vec::new() };
		if self.0.len() < 4 {
			return vec![image(format!("https://i.scdn.co/image/{first}"), Some(640))];
		}
		let mut url = String::with_capacity(192);
		url.push_str("https://mosaic.scdn.co/640/");
		for id in self.0 {
			for byte in id.0 {
				let _ = write!(url, "{byte:02x}");
			}
		}
		vec![image(url, Some(640))]
	}
}

async fn album_covers(session: &Session, raw: &SelectedListContent, mosaic: &mut Mosaic) -> Result<(), librespot_core::Error> {
	let uris: Vec<_> = raw.contents.items.iter().map(|item| item.uri()).filter(|uri| uri.starts_with("spotify:track:")).collect();
	if uris.is_empty() {
		return Ok(());
	}
	let tracks = fetch_metadata::<metadata::Track>(session, &uris, ExtensionKind::TRACK_V4).await?;
	for uri in uris {
		let Some(album) = tracks.get(uri).and_then(|track| track.album.as_ref()) else { continue };
		let covers = if album.cover_group.is_some() { &album.cover_group.image } else { &album.cover };
		if let Some(cover) = covers.iter().filter(|cover| cover.file_id().len() == 20).max_by_key(|cover| {
			cover.width().max(match cover.size() {
				metadata::image::Size::SMALL => 64,
				metadata::image::Size::LARGE => 640,
				metadata::image::Size::XLARGE => 1024,
				metadata::image::Size::DEFAULT => 300,
			})
		}) {
			mosaic.add(FileId::from_raw(cover.file_id()));
		}
		if mosaic.0.len() == 4 {
			break;
		}
	}
	Ok(())
}

pub(super) async fn fill(
	session: &Session,
	info: &mut Playlist,
	first: Option<(&SelectedListContent, &[Track])>,
) -> Result<(), librespot_core::Error> {
	if !info.images.is_empty() || info.track_count == Some(0) {
		return Ok(());
	}
	let mut mosaic = Mosaic::default();
	let mut offset = 0;
	let mut total = info.track_count.unwrap_or(u32::MAX);
	if let Some((raw, tracks)) = first {
		mosaic.rows(tracks);
		offset = u32::try_from(raw.contents.items.len())?;
		total = u32::try_from(raw.length()).unwrap_or_default();
	}
	while mosaic.0.len() < 4 && offset < total {
		let raw = playlist::fetch(session, &info.id, offset, PAGE).await?;
		let native = images(raw.attributes.get_or_default());
		if !native.is_empty() {
			info.images = native;
			return Ok(());
		}
		album_covers(session, &raw, &mut mosaic).await?;
		let length = u32::try_from(raw.contents.items.len())?;
		if length == 0 {
			break;
		}
		offset += length;
		total = u32::try_from(raw.length()).unwrap_or_default();
	}
	info.images = mosaic.finish();
	Ok(())
}

#[cfg(test)]
mod tests {
	use librespot_protocol::playlist4_external::{FormatListAttribute, PictureSize};

	use super::*;

	#[test]
	fn native_picture_formats_support_uploaded_images_mosaics_and_sized_urls() {
		let id = "ab67616d0000b2730123456789abcdef01234567";
		for picture in [id.to_owned(), format!("spotify:image:{id}"), format!("https://i.scdn.co/image/{id}")] {
			assert_eq!(
				images(&ListAttributes { picture: Some(picture.into_bytes()), ..Default::default() })[0].url,
				format!("https://i.scdn.co/image/{id}")
			);
		}
		assert_eq!(
			images(&ListAttributes { picture: Some(vec![0xab; 20]), ..Default::default() })[0].url,
			format!("https://i.scdn.co/image/{}", "ab".repeat(20))
		);
		assert_eq!(url(&format!("spotify:mosaic:{id}:{id}:{id}:{id}")), Some(format!("https://mosaic.scdn.co/640/{}", id.repeat(4))));
		let attributes = ListAttributes {
			picture_size: vec![PictureSize {
				target_name: Some("small".into()),
				url: Some(format!("spotify:image:{id}")),
				..Default::default()
			}],
			..Default::default()
		};
		assert_eq!(images(&attributes)[0].width, Some(64));
		let attributes = ListAttributes {
			format_attributes: vec![FormatListAttribute {
				key: Some("image_url".into()),
				value: Some(format!("https://i.scdn.co/image/{id}")),
				..Default::default()
			}],
			..Default::default()
		};
		assert!(!images(&attributes).is_empty());
	}

	#[test]
	fn mosaic_uses_first_four_distinct_album_covers_in_native_order() {
		let mut mosaic = Mosaic::default();
		for id in [1, 1, 2, 3, 2, 4, 5] {
			mosaic.add(FileId([id; 20]));
		}
		let image = mosaic.finish().remove(0);
		assert_eq!(
			image.url,
			format!("https://mosaic.scdn.co/640/{}{}{}{}", "01".repeat(20), "02".repeat(20), "03".repeat(20), "04".repeat(20))
		);
		assert_eq!(image.width, Some(640));
		assert!(Mosaic::default().finish().is_empty());
	}
}
