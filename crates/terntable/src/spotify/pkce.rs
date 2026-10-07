//! RFC 7636 helpers: S256 challenge, base64url, URL encoding.

use std::{fmt::Write as _, io};
#[cfg(unix)]
use std::{fs::File, io::Read};

use sha2::{Digest, Sha256};

const B64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url (RFC 4648 section 5).
#[expect(clippy::indexing_slicing, reason = "the index is masked to 6 bits and the alphabet has 64 characters")]
pub fn b64url(bytes: &[u8]) -> String {
	let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
	for chunk in bytes.chunks(3) {
		let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
		for i in 0..=chunk.len() {
			out.push(char::from(B64URL[(n >> (18 - 6 * i) & 63) as usize]));
		}
	}
	out
}

/// `BASE64URL(SHA256(ASCII(verifier)))`.
pub fn challenge(verifier: &str) -> String {
	b64url(&Sha256::digest(verifier.as_bytes()))
}

/// `N` bytes from the OS CSPRNG, base64url encoded.
pub fn random_b64url<const N: usize>() -> io::Result<String> {
	let mut buf = [0u8; N];
	#[cfg(unix)]
	File::open("/dev/urandom")?.read_exact(&mut buf)?;
	#[cfg(windows)]
	{
		use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
		// SAFETY: the system CSPRNG fills the supplied writable buffer; no algorithm handle
		// is needed with BCRYPT_USE_SYSTEM_PREFERRED_RNG.
		unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(io::Error::other)?;
	}
	Ok(b64url(&buf))
}

/// A fresh code verifier: 32 random bytes, i.e. 43 unreserved characters (RFC 7636 section 4.1).
pub fn verifier() -> io::Result<String> {
	random_b64url::<32>()
}

/// Percent-encodes everything except RFC 3986 unreserved characters.
pub fn urlencode(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for &b in s.as_bytes() {
		if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
			out.push(char::from(b));
		} else {
			// Writing to a String cannot fail.
			let _ = write!(out, "%{b:02X}");
		}
	}
	out
}

/// Decodes `%XX` escapes and `+` as space (query strings). Invalid escapes pass through.
pub fn urldecode(s: &str) -> String {
	let mut bytes = s.bytes();
	let mut out = Vec::with_capacity(s.len());
	while let Some(b) = bytes.next() {
		match b {
			b'+' => out.push(b' '),
			b'%' => {
				// Look two bytes ahead without consuming them: an invalid escape passes through as it is.
				let mut ahead = bytes.clone();
				if let (Some(hi), Some(lo)) = (ahead.next().and_then(hex), ahead.next().and_then(hex)) {
					out.push(hi << 4 | lo);
					bytes = ahead;
				} else {
					out.push(b'%');
				}
			},
			b => out.push(b),
		}
	}
	String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
	char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rfc7636_appendix_b_vector() {
		// RFC 7636 Appendix B: the verifier is the base64url of this octet sequence,
		// and the S256 challenge of that verifier is given.
		let octets = [
			116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212, 37, 77, 105, 214, 191, 240, 91, 88, 5, 88,
			83, 132, 141, 121,
		];
		let verifier = b64url(&octets);
		assert_eq!(verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
		assert_eq!(challenge(&verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
	}

	#[test]
	fn base64url_lengths_and_alphabet() {
		assert_eq!(b64url(b""), "");
		assert_eq!(b64url(b"f"), "Zg");
		assert_eq!(b64url(b"fo"), "Zm8");
		assert_eq!(b64url(b"foo"), "Zm9v");
		assert_eq!(b64url(&[0xfb, 0xff, 0xfe]), "-__-");
	}

	#[test]
	fn verifier_is_valid_rfc7636() {
		let v = verifier().unwrap();
		assert_eq!(v.len(), 43);
		assert!(v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
		assert_ne!(v, verifier().unwrap());
	}

	#[test]
	fn url_codec_roundtrip() {
		let raw = "http://127.0.0.1:8898/callback?a=b c&é";
		let enc = urlencode(raw);
		assert_eq!(enc, "http%3A%2F%2F127.0.0.1%3A8898%2Fcallback%3Fa%3Db%20c%26%C3%A9");
		assert_eq!(urldecode(&enc), raw);
		assert_eq!(urldecode("a+b%2"), "a b%2");
		assert_eq!(urldecode("%zz"), "%zz");
	}
}
