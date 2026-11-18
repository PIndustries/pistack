// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::Engine as _;

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
