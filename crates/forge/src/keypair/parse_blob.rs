// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::bad::bad;
use super::curve_of::curve_of;
use super::ed25519_pub_len::ED25519_PUB_LEN;
use super::ed25519_type::ED25519_TYPE;
use super::field_bytes_of::field_bytes_of;
use super::min_rsa_bits::MIN_RSA_BITS;
use super::mpint::mpint;
use super::rsa::RSA;
use super::string_field::string_field;

/// Read a public key blob: the key type first, then the parts that type has.
pub(crate) fn parse_blob(bytes: &[u8]) -> ApiResult<String> {
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
