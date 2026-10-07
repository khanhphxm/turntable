//! The collection2v2.proto wire fields (librespot-protocol 0.8 ships the schema but
//! does not generate/export it). Use the existing protobuf runtime rather than a
//! second codec or a fork of all librespot protocol code.

use protobuf::{CodedInputStream, CodedOutputStream};

#[derive(Clone, Copy)]
pub(super) enum Set {
	Saved,
	Artists,
}

impl Set {
	pub(super) const fn name(self) -> &'static str {
		match self {
			Self::Saved => "collection",
			Self::Artists => "artist",
		}
	}
}

#[derive(Clone, Default)]
pub(super) struct CollectionItem {
	pub uri:        String,
	pub added_at:   i64,
	pub is_removed: bool,
}

pub(super) struct PageResponse {
	pub items:           Vec<CollectionItem>,
	pub next_page_token: String,
}

pub(super) fn request(username: &str, set: Set, token: &str) -> protobuf::Result<Vec<u8>> {
	let mut bytes = Vec::with_capacity(username.len() + token.len() + 32);
	{
		let mut out = CodedOutputStream::vec(&mut bytes);
		out.write_string(1, username)?;
		out.write_string(2, set.name())?;
		if !token.is_empty() {
			out.write_string(3, token)?;
		}
		out.write_int32(4, 1000)?;
		out.flush()?;
	}
	Ok(bytes)
}

impl PageResponse {
	pub(super) fn parse_from_bytes(bytes: &[u8]) -> protobuf::Result<Self> {
		let mut input = CodedInputStream::from_bytes(bytes);
		let mut page = Self { items: Vec::new(), next_page_token: String::new() };
		while let Some(tag) = input.read_raw_tag_or_eof()? {
			match tag {
				10 => page.items.push(CollectionItem::parse(&input.read_bytes()?)?),
				18 => page.next_page_token = input.read_string()?,
				_ => protobuf::rt::skip_field_for_tag(tag, &mut input)?,
			}
		}
		Ok(page)
	}
}

impl CollectionItem {
	fn parse(bytes: &[u8]) -> protobuf::Result<Self> {
		let mut input = CodedInputStream::from_bytes(bytes);
		let mut item = Self::default();
		while let Some(tag) = input.read_raw_tag_or_eof()? {
			match tag {
				10 => item.uri = input.read_string()?,
				16 => item.added_at = input.read_int64()?,
				24 => item.is_removed = input.read_bool()?,
				_ => protobuf::rt::skip_field_for_tag(tag, &mut input)?,
			}
		}
		Ok(item)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn request_fields_match_collection2v2() -> protobuf::Result<()> {
		let bytes = request("alice", Set::Saved, "cursor")?;
		let mut input = CodedInputStream::from_bytes(&bytes);
		assert_eq!(input.read_raw_tag_or_eof()?, Some(10));
		assert_eq!(input.read_string()?, "alice");
		assert_eq!(input.read_raw_tag_or_eof()?, Some(18));
		assert_eq!(input.read_string()?, "collection");
		assert_eq!(input.read_raw_tag_or_eof()?, Some(26));
		assert_eq!(input.read_string()?, "cursor");
		assert_eq!(input.read_raw_tag_or_eof()?, Some(32));
		assert_eq!(input.read_int32()?, 1000);
		assert_eq!(input.read_raw_tag_or_eof()?, None);
		Ok(())
	}

	#[test]
	fn artist_follows_use_the_separate_artist_set() -> protobuf::Result<()> {
		let bytes = request("alice", Set::Artists, "")?;
		let mut input = CodedInputStream::from_bytes(&bytes);
		assert_eq!(input.read_raw_tag_or_eof()?, Some(10));
		assert_eq!(input.read_string()?, "alice");
		assert_eq!(input.read_raw_tag_or_eof()?, Some(18));
		assert_eq!(input.read_string()?, "artist");
		assert_eq!(input.read_raw_tag_or_eof()?, Some(32));
		Ok(())
	}

	#[test]
	fn response_keeps_timestamps_removals_cursor_and_skips_unknown_fields() -> protobuf::Result<()> {
		let mut item = Vec::new();
		{
			let mut out = CodedOutputStream::vec(&mut item);
			out.write_string(1, "spotify:track:0000000000000000000001")?;
			out.write_int32(2, 123)?;
			out.write_bool(3, true)?;
			out.write_string(4, "spotify:album:0000000000000000000002")?;
			out.flush()?;
		}
		let mut bytes = Vec::new();
		{
			let mut out = CodedOutputStream::vec(&mut bytes);
			out.write_bytes(1, &item)?;
			out.write_string(2, "next")?;
			out.write_string(3, "sync")?;
			out.flush()?;
		}
		let page = PageResponse::parse_from_bytes(&bytes)?;
		assert_eq!(page.items.len(), 1);
		if let Some(item) = page.items.first() {
			assert_eq!(item.uri, "spotify:track:0000000000000000000001");
			assert_eq!(item.added_at, 123);
			assert!(item.is_removed);
		}
		assert_eq!(page.next_page_token, "next");
		assert!(PageResponse::parse_from_bytes(&[10, 20, 1]).is_err());
		Ok(())
	}
}
