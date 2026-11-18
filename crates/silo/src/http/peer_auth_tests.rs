// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::authorize_peer::authorize_peer;
use axum::http::{HeaderMap, HeaderValue};
use pistack_store::Store;
use pistack_types::error::ErrorCode;

#[test]
fn only_a_member_is_served_unless_the_routes_are_open() {
    let store = Store::in_memory().unwrap();
    store
        .put_meta(gate::token::META_JWT_SECRET, &"c2VjcmV0".to_owned())
        .unwrap();
    let (_tok, clear) = keep::create_join_token(&store, "servers", "admin", true, None).unwrap();
    let mut member = HeaderMap::new();
    member.insert(
        keep::peer::PEER_HEADER,
        HeaderValue::from_str(&keep::peer::peer_key(&store).unwrap()).unwrap(),
    );
    let mut guess = HeaderMap::new();
    guess.insert(keep::peer::PEER_HEADER, HeaderValue::from_static("guess"));
    let mut joining = HeaderMap::new();
    joining.insert(
        pistack_rpc::JOIN_HEADER,
        HeaderValue::from_str(&clear).unwrap(),
    );
    let none = HeaderMap::new();

    for required in [None, Some("1"), Some("")] {
        authorize_peer(&store, &member, required).unwrap();
        for refused in [&none, &guess, &joining] {
            let err = authorize_peer(&store, refused, required).unwrap_err();
            assert_eq!(err.code, ErrorCode::Unauthorized, "{err}");
        }
    }
    authorize_peer(&store, &none, Some(" 0 ")).unwrap();
}
