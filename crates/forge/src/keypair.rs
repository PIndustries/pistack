// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! SSH keys in the formats a guest already takes.
//!
//! A key is stored as the OpenSSH line the guest receives, so what the API
//! keeps is byte for byte what cloud-init writes. `ed25519`, `ecdsa` and
//! `rsa` are read from their wire blobs; `dsa` and anything malformed is
//! refused. An `rsa` key must be 2048 bits or larger.
//!
//! An `ed25519` pair can also be made here. The private half leaves through
//! the create response in OpenSSH's private key format and is not stored.

use base64::Engine as _;
use pistack_types::{ApiError, ApiResult};
use rand::RngCore;
use ring::signature::KeyPair as _;
use sha2::{Digest, Sha256};

/// Key types the API accepts, by their wire name.
pub const ED25519_TYPE: &str = "ssh-ed25519";
pub const ECDSA_P256: &str = "ecdsa-sha2-nistp256";
pub const ECDSA_P384: &str = "ecdsa-sha2-nistp384";
pub const ECDSA_P521: &str = "ecdsa-sha2-nistp521";
pub const RSA: &str = "ssh-rsa";

/// Bytes of an `ed25519` public key, and of the seed.
const ED25519_PUB_LEN: usize = 32;
const ED25519_SEED_LEN: usize = 32;
/// The bytes of the private half: seed then public key.
const ED25519_PRIV_LEN: usize = 64;
const MIN_RSA_BITS: u64 = 2048;

/// A public key the API accepted.
pub struct PublicKey {
    /// Wire name: `ssh-ed25519`, `ssh-rsa`, `ecdsa-sha2-nistp256`.
    pub key_type: String,
    /// The key as one OpenSSH line, comment included when it had one.
    pub line: String,
    /// `SHA256:...`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
}

/// Base64 that accepts padding or its absence; both appear in key files.
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );

/// The same alphabet without the padding `ssh-keygen -l` leaves off.
const B64_NOPAD: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::general_purpose::GeneralPurposeConfig::new()
            .with_encode_padding(false)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::bad_request(msg)
}

/// A length-prefixed field, the only thing a wire blob is made of.
fn string_field<'a>(blob: &'a [u8], at: &mut usize, what: &str) -> ApiResult<&'a [u8]> {
    if *at + 4 > blob.len() {
        return Err(bad(format!("{what} is missing")));
    }
    let len = u32::from_be_bytes([blob[*at], blob[*at + 1], blob[*at + 2], blob[*at + 3]]) as usize;
    if len > blob.len() - *at - 4 {
        return Err(bad(format!("{what} is truncated")));
    }
    *at += 4;
    let v = &blob[*at..*at + len];
    *at += len;
    Ok(v)
}

/// A big-endian unsigned integer. A leading zero byte is the sign, which no
/// key part needs, and a high bit without it is a negative number.
fn mpint<'a>(blob: &'a [u8], at: &mut usize, what: &str) -> ApiResult<&'a [u8]> {
    let raw = string_field(blob, at, what)?;
    let (head, digits) = raw
        .split_first()
        .ok_or_else(|| bad(format!("{what} is empty")))?;
    if *head & 0x80 != 0 {
        // A negative number is not a key part. The sign is a leading zero,
        // which the caller measures against, so it stays in what is returned.
        return Err(bad(format!("{what} is not a positive integer")));
    }
    if digits.is_empty() {
        return Err(bad(format!("{what} is empty")));
    }
    Ok(raw)
}

fn ssh_string(field: &[u8]) -> Vec<u8> {
    let mut out = (field.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(field);
    out
}

/// The SHA256 fingerprint of a public key blob, as `ssh-keygen -l` prints it:
/// all 32 bytes of the digest in unpadded base64, which is 43 characters.
fn fingerprint(blob: &[u8]) -> String {
    let digest = Sha256::digest(blob);
    format!("SHA256:{}", B64_NOPAD.encode(digest))
}

/// The `ecdsa` curve an accepted key type names.
fn curve_of(key_type: &str) -> Option<&'static str> {
    match key_type {
        ECDSA_P256 => Some("nistp256"),
        ECDSA_P384 => Some("nistp384"),
        ECDSA_P521 => Some("nistp521"),
        _ => None,
    }
}

/// Bits of the field of an accepted `ecdsa` curve.
fn field_bytes_of(key_type: &str) -> usize {
    match key_type {
        ECDSA_P256 => 32,
        ECDSA_P384 => 48,
        _ => 66,
    }
}

/// Read a public key blob: the key type first, then the parts that type has.
fn parse_blob(bytes: &[u8]) -> ApiResult<String> {
    let mut at = 0usize;
    let key_type = String::from_utf8(string_field(bytes, &mut at, "key type")?.to_vec())
        .map_err(|_| bad("public key is not valid OpenSSH"))?;
    match key_type.as_str() {
        ED25519_TYPE => {
            let key = string_field(bytes, &mut at, "public key")?;
            if key.len() != ED25519_PUB_LEN || at != bytes.len() {
                return Err(bad("ssh-ed25519 public key is malformed"));
            }
        }
        name if curve_of(name).is_some() => {
            let curve = string_field(bytes, &mut at, "curve")?;
            if curve != curve_of(name).unwrap().as_bytes() {
                return Err(bad(format!("{name} names a curve it is not")));
            }
            let point = string_field(bytes, &mut at, "point")?;
            // An uncompressed point: 0x04, then x and y of one field each.
            let field = field_bytes_of(name);
            if point.len() != 1 + 2 * field
                || point[0] != 0x04
                || point[1..].iter().all(|b| *b == 0)
                || at != bytes.len()
            {
                return Err(bad(format!("{name} public key is malformed")));
            }
        }
        RSA => {
            let e = mpint(bytes, &mut at, "rsa exponent")?;
            let n = mpint(bytes, &mut at, "rsa modulus")?;
            if at != bytes.len() {
                return Err(bad("ssh-rsa public key is malformed"));
            }
            // The high bit of a modulus would read as a negative number, so
            // a key of `bits` takes `bits / 8` bytes, or one more when it
            // needs the sign byte that keeps it positive.
            let n = &n[usize::from(n.first() == Some(&0))..];
            let bits = n.len() as u64 * 8;
            if bits < MIN_RSA_BITS {
                return Err(bad(format!(
                    "ssh-rsa key is {bits} bits; {} or more is required",
                    MIN_RSA_BITS
                )));
            }
            if e != [1, 0, 1] && e != [0, 3] && e != [3] {
                return Err(bad("ssh-rsa exponent is neither 65537 nor 3"));
            }
        }
        other => {
            return Err(bad(format!(
                "{other} is not accepted; use ssh-ed25519, ecdsa-sha2-nistp256/384/521 or ssh-rsa"
            )));
        }
    }
    Ok(key_type)
}

/// Read one OpenSSH public key line, the `<type> <base64> [comment]` form
/// `authorized_keys` uses. A key with no comment is refused: every real one
/// carries one, and requiring it turns a line cut short into an error
/// instead of a key nobody can name later.
pub fn parse(line: &str) -> ApiResult<PublicKey> {
    if line.lines().count() > 1 {
        return Err(bad("public key must be one line"));
    }
    // A quoted `authorized_keys` option list comes before the key.
    let line = match line.trim() {
        quoted if quoted.starts_with('"') => quoted
            .strip_prefix('"')
            .and_then(|r| r.split_once('"'))
            .map(|(_, after)| after.trim_start())
            .ok_or_else(|| bad("public key option list is not closed"))?,
        other => other,
    };
    let (prefix, rest) = line
        .split_once(char::is_whitespace)
        .ok_or_else(|| bad("public key must be `type base64 [comment]` on one line"))?;
    let rest = rest.trim_start();
    let (b64, comment) = match rest.find(char::is_whitespace) {
        Some(i) => (&rest[..i], rest[i..].trim()),
        None => (rest, ""),
    };
    if comment.is_empty() {
        return Err(bad(
            "public key must be `type base64 [comment]` on one line",
        ));
    }
    if line.chars().any(char::is_control) {
        return Err(bad("public key must not contain control characters"));
    }
    // Padding is optional on the wire, so a length that is not a multiple of
    // four is only valid without it.
    if b64.len() % 4 != 0 && b64.contains('=') {
        return Err(bad("public key is not valid base64"));
    }
    let blob = B64
        .decode(b64)
        .map_err(|_| bad("public key is not valid base64"))?;

    let key_type = parse_blob(&blob)?;
    if prefix != key_type {
        return Err(bad("public key type does not match the key it carries"));
    }
    Ok(PublicKey {
        key_type: key_type.clone(),
        line: format!("{key_type} {b64} {comment}"),
        fingerprint: fingerprint(&blob),
    })
}

fn random(n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut out);
    out
}

/// A generated `ed25519` pair: the public half to store, and the private key
/// once in OpenSSH's private key format.
pub struct Generated {
    pub public: PublicKey,
    pub private_openssh: String,
}

/// An `ed25519` key pair. `ring` makes the seed and derives the public half;
/// the private key is then laid out the way `ssh-keygen` writes one.
pub fn generate_ed25519(comment: &str) -> Generated {
    let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
        .expect("ed25519 pkcs8 from a fresh key");
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .expect("the pkcs8 just generated");
    let public_bytes = pair.public_key().as_ref().to_vec();
    // The seed is the last 32 bytes of the wrapper and nothing else.
    let seed = pkcs8.as_ref()[pkcs8.as_ref().len() - ED25519_SEED_LEN..].to_vec();

    let mut blob = Vec::new();
    blob.extend_from_slice(&ssh_string(ED25519_TYPE.as_bytes()));
    blob.extend_from_slice(&ssh_string(&public_bytes));
    let public = PublicKey {
        key_type: ED25519_TYPE.to_owned(),
        line: format!(
            "{ED25519_TYPE} {}{}",
            base64::engine::general_purpose::STANDARD.encode(&blob),
            if comment.is_empty() {
                String::new()
            } else {
                format!(" {comment}")
            }
        ),
        fingerprint: fingerprint(&blob),
    };

    // What an unencrypted single-key file holds after its length: two
    // identical check ints, the key type, the seed, the seed with its public
    // half, then the comment.
    let privkey = [seed.as_slice(), public_bytes.as_slice()].concat();
    debug_assert_eq!(privkey.len(), ED25519_PRIV_LEN);
    let check = random(4);
    let mut privbody = Vec::new();
    privbody.extend_from_slice(&check);
    privbody.extend_from_slice(&check);
    privbody.extend_from_slice(&ssh_string(ED25519_TYPE.as_bytes()));
    privbody.extend_from_slice(&ssh_string(&seed));
    privbody.extend_from_slice(&ssh_string(&privkey));
    privbody.extend_from_slice(&ssh_string(comment.as_bytes()));
    // Padding counts up from 1 to the next multiple of the cipher block, and
    // the length covers the body and its padding.
    let pad = 8 - (privbody.len() % 8) % 8;
    privbody.extend((1..=pad).map(|i| i as u8));
    let privblob = privbody;

    let mut body = Vec::new();
    body.extend_from_slice(b"openssh-key-v1\0");
    body.extend_from_slice(&ssh_string(b"none"));
    body.extend_from_slice(&ssh_string(b"none"));
    body.extend_from_slice(&ssh_string(b""));
    body.extend(&1u32.to_be_bytes());
    body.extend_from_slice(&ssh_string(&blob));
    body.extend_from_slice(&ssh_string(&privblob));

    // The body is padded so its base64 needs no `=`; the public line is
    // written the way `ssh-keygen` writes it, padding and all.
    let mut pem = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&body);
    for chunk in encoded.as_bytes().chunks(70) {
        pem.push_str(&String::from_utf8_lossy(chunk));
        pem.push('\n');
    }
    pem.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    Generated {
        public,
        private_openssh: pem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6NiWNsbUCDY1DqQxFpj7Y1p+Ph+ovvRShuoREjxj/7 demo@box";

    /// A key made by `ssh-keygen -t ed25519`, and the fingerprint
    /// `ssh-keygen -l` printed for it. A digest short by one byte still looks
    /// like a fingerprint, so the real tool's answer is what pins this down.
    const KEYGEN_LINE: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA8HlL+M+dixOjpEz4FBtUp5MsL7tvYq5uDW/ek/IZq9 fpcheck@pistack";
    const KEYGEN_FINGERPRINT: &str = "SHA256:B8lQMvxPkRTmaSDKqTQngn3j6uORoRTBd3Y0JLbCdFY";

    #[test]
    fn reads_an_ed25519_line() {
        let k = parse(LINE).unwrap();
        assert_eq!(k.key_type, ED25519_TYPE);
        assert_eq!(k.line, LINE);
        assert_eq!(
            k.fingerprint,
            "SHA256:kZaHsQEOwFK+9RD5E9L3VAB8FaS9MZYfNAMVbxg0f1I"
        );
    }

    #[test]
    fn fingerprint_matches_ssh_keygen() {
        let k = parse(KEYGEN_LINE).unwrap();
        assert_eq!(k.fingerprint, KEYGEN_FINGERPRINT);
        // All 32 digest bytes, unpadded: 43 characters after `SHA256:`.
        assert_eq!(k.fingerprint.len(), "SHA256:".len() + 43);
        let k = parse(LINE).unwrap();
        assert_eq!(k.fingerprint.len(), "SHA256:".len() + 43);
    }

    #[test]
    fn surrounding_space_is_not_part_of_the_key() {
        let k = parse(&format!("  {LINE}\n")).unwrap();
        assert_eq!(k.line, LINE);
    }

    #[test]
    fn refuses_what_is_not_a_usable_key() {
        for bad in [
            "",
            "ssh-ed25519",
            "ssh-ed25519 AAAA",
            "ssh-ed25519 AAAA box",
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6NiWNsbUCDY1DqQxFpj7Y1p+Ph+ovvRShuoREjxj/7",
            "ssh-ed25519!!! AAAA box",
            "ssh-ed25519 AAAA==AAA= box",
            "ssh-ed25519 AAAA box\nssh-ed25519 AAAA box",
            "ssh-ed25519 AAA\tA box",
            // dsa is not accepted at any length
            "ssh-dss AAAAB3NzaC1kc3MAAACB dummy@x",
            // an ed25519 key body of the wrong length
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA== dummy@x",
        ] {
            assert!(parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn a_made_pair_is_a_pair() {
        let g = generate_ed25519("made@box");
        assert_eq!(g.public.key_type, ED25519_TYPE);
        assert!(g.public.line.ends_with(" made@box"), "{}", g.public.line);
        // The line the API would store reads back as itself.
        let again = parse(&g.public.line).unwrap();
        assert_eq!(again.line, g.public.line);
        assert_eq!(again.fingerprint, g.public.fingerprint);
        // The private half is the OpenSSH format, and different every time.
        assert!(g
            .private_openssh
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
        assert!(g
            .private_openssh
            .trim_end()
            .ends_with("-----END OPENSSH PRIVATE KEY-----"));
        let other = generate_ed25519("made@box");
        assert_ne!(other.private_openssh, g.private_openssh);
        assert_ne!(other.public.line, g.public.line);
    }

    /// The public key inside a private key file, from its own bytes.
    fn public_from_private(pem: &str) -> Vec<u8> {
        let body = base64::engine::general_purpose::STANDARD
            .decode(
                pem.lines()
                    .filter(|l| !l.starts_with("-----"))
                    .collect::<String>(),
            )
            .unwrap();
        assert!(body.starts_with(b"openssh-key-v1\0"));
        // cipher, kdf, kdf options: three names. Then the key count, the
        // public blob, and the private part with a length of its own.
        let mut at = b"openssh-key-v1\0".len();
        let field = |at: &mut usize| {
            let n = u32::from_be_bytes(body[*at..*at + 4].try_into().unwrap()) as usize;
            *at += 4;
            let v = body[*at..*at + n].to_vec();
            *at += n;
            v
        };
        assert_eq!(field(&mut at), b"none");
        assert_eq!(field(&mut at), b"none");
        assert!(field(&mut at).is_empty());
        assert_eq!(body[at..at + 4], [0, 0, 0, 1]);
        at += 4;
        let public_blob = field(&mut at);
        let private = body[at..].to_vec();
        let declared = u32::from_be_bytes(private[..4].try_into().unwrap()) as usize;
        let inner = &private[4..4 + declared];
        assert_eq!(&inner[0..4], &inner[4..8], "the check ints differ");
        let mut q = 8;
        let part = |q: &mut usize| {
            let n = u32::from_be_bytes(inner[*q..*q + 4].try_into().unwrap()) as usize;
            *q += 4;
            let v = inner[*q..*q + n].to_vec();
            *q += n;
            v
        };
        let key_type = part(&mut q);
        let seed = part(&mut q);
        let key = part(&mut q);
        let comment = part(&mut q);
        assert_eq!(key_type, ED25519_TYPE.as_bytes());
        // The public half is the tail of the private one, and is the blob the
        // public line carries.
        assert_eq!(
            &public_blob[4 + ED25519_TYPE.len() + 4..],
            &key[ED25519_SEED_LEN..]
        );
        assert_eq!(seed.len(), ED25519_SEED_LEN);
        assert_eq!(key.len(), ED25519_PRIV_LEN);
        assert_eq!(comment, b"made@box");
        public_blob
    }

    #[test]
    fn a_made_private_key_carries_the_key_it_claims() {
        let g = generate_ed25519("made@box");
        assert_eq!(
            public_from_private(&g.private_openssh),
            public_blob(&g.public)
        );
        assert_eq!(
            fingerprint(&public_from_private(&g.private_openssh)),
            g.public.fingerprint
        );
    }

    fn public_blob(key: &PublicKey) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(key.line.split_whitespace().nth(1).unwrap())
            .unwrap()
    }
}
