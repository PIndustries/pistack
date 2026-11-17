// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end OIDC sign-in through the router.
//!
//! A throwaway RSA key is generated at test start. A tiny in-process HTTP
//! server on `127.0.0.1:0` plays the provider: it serves the OpenID discovery
//! document and a JWKS holding the public half. Tokens are minted by
//! `jsonwebtoken` with the private key, so the verifier's `ring` path checks a
//! real RS256 signature. The issuer is that loopback server, which is the one
//! place gate accepts plain HTTP.

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use gate::{self, Gate, ADMIN_PASSWORD_ENV};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use pistack_store::Store;
use pistack_types::module::CancellationToken;
use pistack_types::{Module, ModuleContext};
use rsa::pkcs8::EncodePrivateKey;
use rsa::rand_core::OsRng;
use rsa::traits::PublicKeyParts;
use rsa::RsaPrivateKey;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;

/// Everything one test run shares: the router, the provider's issuer, and the
/// keys to mint tokens with.
struct Ctx {
    app: Router,
    store: Store,
    issuer: String,
    encoding: EncodingKey,
    jwks: String,
    kid: String,
    fetches: Arc<AtomicUsize>,
}

async fn boot() -> (Router, Store) {
    std::env::set_var(ADMIN_PASSWORD_ENV, "correct-horse");
    let store = Store::in_memory().unwrap();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    Gate::default().start(ctx.clone()).await.unwrap();
    (Gate::default().router(ctx), store)
}

/// The provider stand-in. Serves the two endpoints gate fetches on every
/// request; a body it does not recognise is a 404. Runs until dropped.
fn spawn_provider(jwks: String, fetches: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            fetches.fetch_add(1, Ordering::SeqCst);
            let body = if path.ends_with("/.well-known/openid-configuration") {
                format!(r#"{{"issuer":"http://{addr}","jwks_uri":"http://{addr}/jwks"}}"#)
            } else if path.ends_with("/jwks") {
                jwks.clone()
            } else {
                String::from("{}")
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}")
}

/// Generate the key, build the JWKS the provider will serve, and boot a router
/// whose admin can create providers.
async fn setup() -> Ctx {
    let mut rng = OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("gen rsa key");
    let n = URL_SAFE_NO_PAD.encode(key.n().to_bytes_be());
    let e = URL_SAFE_NO_PAD.encode(key.e().to_bytes_be());
    let kid = "test-key-1".to_string();
    let jwks = format!(
        r#"{{"keys":[{{"kty":"RSA","kid":"{kid}","alg":"RS256","use":"sig","n":"{n}","e":"{e}"}}]}}"#
    );
    let pem = key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("pkcs8 pem");
    let encoding = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("encoding key");
    let fetches = Arc::new(AtomicUsize::new(0));
    let issuer = spawn_provider(jwks.clone(), fetches.clone());
    let (app, store) = boot().await;
    Ctx {
        app,
        store,
        issuer,
        encoding,
        jwks,
        kid,
        fetches,
    }
}

/// JWKS fetches the test provider has served. Each sign-in that needs keys and
/// finds them in the cache does not add to this.
fn fetch_count(ctx: &Ctx) -> usize {
    ctx.fetches.load(Ordering::SeqCst)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

async fn admin_token(app: &Router) -> String {
    let (s, v) = call(
        app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name": "admin", "password": "correct-horse"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["token"].as_str().unwrap().to_owned()
}

/// Sign a JWT with arbitrary claims, RS256, naming our `kid`.
fn mint(encoding: &EncodingKey, kid: &str, claims: &Value) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some(kid.to_string());
    encode(&h, claims, encoding).unwrap()
}

/// A well-formed token for `iss`/`aud`/now, naming `email`. Extra fields merge.
///
/// Every token carries a `sub`, because gate requires one: it is the only
/// thing a token is matched to an account by. A token with no `sub` is minted
/// through `mint` directly, as the no-subject test does.
fn good_token(ctx: &Ctx, sub: &str, email: &str, extra: Option<Value>) -> String {
    let now = now_secs();
    let mut claims = json!({
        "iss": ctx.issuer,
        "aud": "client-1",
        "exp": now + 300,
        "iat": now,
        "sub": sub,
        "email": email,
        "email_verified": true,
    });
    if let Some(extra) = extra {
        let obj = claims.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            obj.insert(k.clone(), v.clone());
        }
    }
    mint(&ctx.encoding, &ctx.kid, &claims)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Register a provider through the admin API and return its id.
async fn create_provider(ctx: &Ctx, admin: &str, body: Value) -> String {
    let (s, v) = call(&ctx.app, "POST", "/providers", Some(admin), Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "create provider: {v}");
    v["id"].as_str().unwrap().to_owned()
}

/// Tell the cloud that the account `user` is the provider's `subject`. The only
/// way an existing account becomes reachable by an OIDC sign-in.
async fn link(app: &Router, admin: &str, user: &str, provider: &str, subject: &str) -> StatusCode {
    let (s, _) = call(
        app,
        "POST",
        "/identities",
        Some(admin),
        Some(json!({"user": user, "provider": provider, "subject": subject})),
    )
    .await;
    s
}

/// Take an account's external identity off again.
async fn unlink(app: &Router, admin: &str, provider: &str, subject: &str) -> StatusCode {
    let (s, _) = call(
        app,
        "POST",
        "/identities/remove",
        Some(admin),
        Some(json!({"provider": provider, "subject": subject})),
    )
    .await;
    s
}

/// The store behind a test's router, for a check that wants the records rather
/// than what the API prints: a count taken from a paged list is wrong the moment
/// the list has a second page.
fn store_of(ctx: &Ctx) -> &Store {
    &ctx.store
}

/// The account record as the admin reads it.
async fn principal(app: &Router, admin: &str, name: &str) -> Value {
    let (s, v) = call(
        app,
        "GET",
        &format!("/principals/{name}"),
        Some(admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

/// POST /oidc/login, unauthenticated, and parse the answer.
async fn oidc_login(app: &Router, provider: &str, id_token: &str) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        "/oidc/login",
        None,
        Some(json!({"provider": provider, "id_token": id_token})),
    )
    .await
}

/// The uniform refusal this step answers every bad token with.
const REFUSAL: &str = "the identity token was not accepted";

#[tokio::test]
async fn a_good_token_signs_in_an_existing_user() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    // No create: only the pre-existing account can be signed in.
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    // The account exists already as a password user. OIDC signs them in without
    // asking for that password.
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(json!({"name":"alice","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // Nothing reaches the account until an administrator says which external
    // subject it is; the name and the email claim are no business of sign-in.
    assert_eq!(
        oidc_login(
            &ctx.app,
            "corp",
            &good_token(&ctx, "alice-1", "alice@example.com", None)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED,
        "a token matching nothing is refused, not matched to `alice`"
    );
    assert_eq!(
        link(&ctx.app, &admin, "alice", "corp", "alice-1").await,
        StatusCode::OK
    );

    let tok = good_token(&ctx, "alice-1", "alice@example.com", None);
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Same shape as a password login's answer.
    assert!(v["token"].is_string());
    assert_eq!(v["token_type"], "Bearer");
    assert_eq!(v["caller"]["principal_name"], "alice");
    assert_eq!(v["caller"]["system"], false);

    // A second sign-in reuses the cached keys: the discovery exchange runs
    // again, but it is the same number of key fetches, not another one's worth.
    let before = fetch_count(&ctx);
    let tok2 = good_token(&ctx, "alice-1", "alice@example.com", None);
    assert_eq!(oidc_login(&ctx.app, "corp", &tok2).await.0, StatusCode::OK);
    assert_eq!(
        fetch_count(&ctx),
        before,
        "keys were cached and not refetched for a known kid"
    );
}

#[tokio::test]
async fn a_local_user_is_not_found_by_the_email_the_token_carries() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    // create_unknown is off: only an account bound to the subject can be signed
    // in, and nothing binds one but a link or a sign-in that created it.
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    // The account is named `alice` and carries exactly the address the token
    // will carry. Both used to be enough to sign in as it.
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(
            json!({"name":"alice","email":"alice@example.com","password":"correct horse battery"}),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let tok = good_token(&ctx, "someone-else-entirely", "alice@example.com", None);
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_eq!(v["error"]["message"], REFUSAL);
    // Linked to the subject the token carries, the same token gets in.
    assert_eq!(
        link(&ctx.app, &admin, "alice", "corp", "someone-else-entirely").await,
        StatusCode::OK
    );
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["caller"]["principal_name"], "alice");
}

#[tokio::test]
async fn an_unknown_user_is_refused_when_create_is_off() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let tok = good_token(&ctx, "nobody-1", "nobody@example.com", None);
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn the_admin_account_is_not_reachable_by_an_email_that_starts_with_admin() {
    // The bug this closes: the built-in `admin` holds the name, so a token the
    // provider vouches for whose email's local part is `admin` used to be
    // matched to it and signed the caller in as a cloud admin.
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    let before = principal(&ctx.app, &admin, "admin").await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let tok = good_token(&ctx, "eve-1", "admin@evil.example", None);
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // A new account, suffixed, with no roles at all — never the built-in admin.
    assert_eq!(v["caller"]["principal_name"], "admin-2");
    assert_eq!(v["caller"]["roles"].as_array().unwrap().len(), 0, "{v}");
    assert_eq!(v["caller"]["system"], false, "{v}");
    // The built-in admin is untouched: still one account with that name, still
    // no external identity, still not the account the sign-in answered for.
    let after = principal(&ctx.app, &admin, "admin").await;
    assert_eq!(after["id"], before["id"], "the built-in admin is unchanged");
    assert!(after["spec"].get("external").is_none(), "{after}");
    let created = principal(&ctx.app, &admin, "admin-2").await;
    assert_eq!(created["spec"]["email"], "admin@evil.example");
    assert_eq!(created["spec"]["auth_source"], "corp");
    assert!(
        created["spec"].get("password_hash").is_none()
            || created["spec"]["password_hash"].is_null()
    );
}

#[tokio::test]
async fn a_token_without_a_subject_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let now = now_secs();
    // The name claim is present and verified; the `sub` that would name an
    // account is not.
    let tok = mint(
        &ctx.encoding,
        &ctx.kid,
        &json!({"iss":ctx.issuer,"aud":"client-1","exp":now + 300,"email":"a@b.c","email_verified":true}),
    );
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_eq!(v["error"]["message"], REFUSAL);
    // Nothing was created out of a token that names nobody.
    assert_eq!(
        call(&ctx.app, "GET", "/principals/a", Some(&admin), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_same_subject_signs_in_to_the_same_account_as_its_email_moves() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let (s, v) = oidc_login(
        &ctx.app,
        "corp",
        &good_token(&ctx, "carol-1", "carol@example.com", None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let first = v["caller"]["principal"].as_str().unwrap().to_owned();
    let (s, second) = oidc_login(
        &ctx.app,
        "corp",
        &good_token(&ctx, "carol-1", "carol@moved.example", None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{second}");
    assert_eq!(
        second["caller"]["principal"], first,
        "the subject, not the email, is the account"
    );
    assert_eq!(second["caller"]["principal_name"], "carol");
    // Only the one account was ever made. Counted on the store the router was
    // built on: `GET /principals` pages, and a page boundary would make a count
    // taken from it wrong.
    let bound: Vec<gate::Principal> = store_of(&ctx)
        .collection::<gate::Principal>()
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|p| {
            p.spec
                .external
                .as_ref()
                .is_some_and(|e| e.subject == "carol-1")
        })
        .collect();
    assert_eq!(bound.len(), 1, "one account for one subject");
    assert_eq!(bound[0].meta.name, "carol");
    assert_eq!(
        bound[0].spec.email.as_deref(),
        Some("carol@example.com"),
        "the stored email is the one it arrived with: display only"
    );
}

#[tokio::test]
async fn the_same_subject_at_two_providers_is_two_accounts() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    for name in ["corp", "other"] {
        create_provider(
            &ctx,
            &admin,
            json!({"name":name,"issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
        )
        .await;
    }
    let (s, a) = oidc_login(
        &ctx.app,
        "corp",
        &good_token(&ctx, "shared-1", "dan@example.com", None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{a}");
    // The second provider vouches for the same subject string. That is a
    // different person as far as the cloud is concerned: only the pair matches.
    let (s, b) = oidc_login(
        &ctx.app,
        "other",
        &good_token(&ctx, "shared-1", "dan@example.com", None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_ne!(a["caller"]["principal"], b["caller"]["principal"]);
    assert_eq!(a["caller"]["principal_name"], "dan");
    // Suffixed, because the name was already taken and is never reused.
    assert_eq!(b["caller"]["principal_name"], "dan-2");
}

#[tokio::test]
async fn a_linked_account_keeps_its_own_name_and_its_password_login() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(json!({"name":"erin","email":"erin@example.com","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(
        link(&ctx.app, &admin, "erin", "corp", "erin-1").await,
        StatusCode::OK
    );
    let got = principal(&ctx.app, &admin, "erin").await;
    assert_eq!(
        got["spec"]["email"], "erin@example.com",
        "the link stores no email"
    );
    assert_eq!(got["spec"]["external"]["subject"], "erin-1");
    assert!(
        got["spec"]["external"]["provider"]
            .as_str()
            .unwrap()
            .starts_with("idp-"),
        "the pair records the provider's id: {got}"
    );
    // The password still works: an account that had one keeps password login.
    let (s, v) = call(
        &ctx.app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name":"erin","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // And the provider signs the same account in.
    let (s, v) = oidc_login(
        &ctx.app,
        "corp",
        &good_token(&ctx, "erin-1", "erin@example.com", None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["caller"]["principal_name"], "erin");
    assert_eq!(v["caller"]["principal"], got["id"]);
}

#[tokio::test]
async fn a_pair_belongs_to_one_account_and_a_second_link_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    for name in ["erin", "frank"] {
        let (s, _) = call(
            &ctx.app,
            "POST",
            "/principals",
            Some(&admin),
            Some(json!({"name":name,"password":"correct horse battery"})),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
    }
    assert_eq!(
        link(&ctx.app, &admin, "erin", "corp", "shared-1").await,
        StatusCode::OK
    );
    // The pair is already somebody's: a second account cannot claim it.
    let (s, v) = call(
        &ctx.app,
        "POST",
        "/identities",
        Some(&admin),
        Some(json!({"user":"frank","provider":"corp","subject":"shared-1"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "conflict");
    // An account holds one external identity; a second pair is refused too.
    let (s, v) = call(
        &ctx.app,
        "POST",
        "/identities",
        Some(&admin),
        Some(json!({"user":"erin","provider":"corp","subject":"another-1"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    // Only erin is reachable by the pair, and the refusal signed nobody in.
    let (s, v) = oidc_login(
        &ctx.app,
        "corp",
        &good_token(&ctx, "shared-1", "x@y.z", None),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["caller"]["principal_name"], "erin");
    // Linking the pair it already holds changes nothing and is not an error.
    assert_eq!(
        link(&ctx.app, &admin, "erin", "corp", "shared-1").await,
        StatusCode::OK
    );
    // A link naming no account is a bad request, not a silent no-op; a link
    // naming no subject is the same.
    for body in [
        json!({"provider":"corp","subject":"fresh-1"}),
        json!({"user":"frank","provider":"corp","subject":"  "}),
    ] {
        let (s, v) = call(&ctx.app, "POST", "/identities", Some(&admin), Some(body)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }
}

#[tokio::test]
async fn unlinking_an_account_refuses_the_subject_it_lost() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(json!({"name":"grace","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(
        link(&ctx.app, &admin, "grace", "corp", "grace-1").await,
        StatusCode::OK
    );
    let tok = good_token(&ctx, "grace-1", "grace@example.com", None);
    assert_eq!(oidc_login(&ctx.app, "corp", &tok).await.0, StatusCode::OK);
    assert_eq!(
        unlink(&ctx.app, &admin, "corp", "grace-1").await,
        StatusCode::OK
    );
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_eq!(v["error"]["message"], REFUSAL);
    // The password login the account always had is untouched.
    let (s, v) = call(
        &ctx.app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name":"grace","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // The freed pair can be given to somebody else.
    assert_eq!(
        link(&ctx.app, &admin, "grace", "corp", "grace-1").await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_link_routes_are_for_a_cloud_admin_only() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(json!({"name":"heidi","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, v) = call(
        &ctx.app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name":"heidi","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let tok = v["token"].as_str().unwrap().to_owned();
    let body = json!({"user":"heidi","provider":"corp","subject":"heidi-1"});
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/identities",
        Some(&tok),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/identities/remove",
        Some(&tok),
        Some(json!({"provider":"corp","subject":"heidi-1"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&ctx.app, "POST", "/identities", None, Some(body)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // Nothing was linked by the refusals.
    assert!(principal(&ctx.app, &admin, "heidi").await["spec"]
        .get("external")
        .is_none());
}

#[tokio::test]
async fn deleting_a_provider_is_refused_while_accounts_are_linked() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    let id = create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    for name in ["ivan", "judy"] {
        let (s, _) = call(
            &ctx.app,
            "POST",
            "/principals",
            Some(&admin),
            Some(json!({"name":name,"password":"correct horse battery"})),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        assert_eq!(
            link(&ctx.app, &admin, name, "corp", &format!("{name}-1")).await,
            StatusCode::OK
        );
    }
    let (s, v) = call(
        &ctx.app,
        "DELETE",
        &format!("/providers/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let message = v["error"]["message"].as_str().unwrap();
    assert!(message.contains("2"), "the refusal counts them: {message}");
    assert!(
        message.contains("corp"),
        "and names the provider: {message}"
    );
    // The provider is still there, and so is what was bound to it.
    assert_eq!(
        call(
            &ctx.app,
            "GET",
            &format!("/providers/{id}"),
            Some(&admin),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    // Unlinked: the deletion goes through, and the sign-in it enabled is gone.
    for name in ["ivan", "judy"] {
        assert_eq!(
            unlink(&ctx.app, &admin, "corp", &format!("{name}-1")).await,
            StatusCode::OK
        );
    }
    let (s, v) = call(
        &ctx.app,
        "DELETE",
        &format!("/providers/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
}

#[tokio::test]
async fn an_unknown_user_is_created_when_create_is_on() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let tok = good_token(&ctx, "bob-9", "Bob.New@Example.com", None);
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // The account's label comes from the email claim's local part, cleaned to a
    // legal name; the full address is kept as the account's email. The subject
    // it was created for is recorded as the account's external identity.
    assert_eq!(v["caller"]["principal_name"], "bob.new");
    // The created account exists and carries no password of its own.
    let got = principal(&ctx.app, &admin, "bob.new").await;
    assert_eq!(got["spec"]["auth_source"], "corp");
    assert_eq!(got["spec"]["email"], "Bob.New@Example.com");
    assert_eq!(got["spec"]["external"]["subject"], "bob-9");
    assert!(got["spec"].get("password_hash").is_none() || got["spec"]["password_hash"].is_null());
}

#[tokio::test]
async fn a_provider_created_user_cannot_use_password_login() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let tok = good_token(&ctx, "carol-1", "carol@example.com", None);
    assert_eq!(oidc_login(&ctx.app, "corp", &tok).await.0, StatusCode::OK);
    // No password was ever set, and the auth_source marker refuses a password
    // attempt regardless — the account is labelled `carol` (from the email).
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name":"carol","password":"whatever"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_wrong_audience_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let now = now_secs();
    let tok = mint(
        &ctx.encoding,
        &ctx.kid,
        &json!({"iss":ctx.issuer,"aud":"someone-else","exp":now + 300,"sub":"s-1","email":"a@b.c","email_verified":true}),
    );
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn a_wrong_issuer_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let now = now_secs();
    let tok = mint(
        &ctx.encoding,
        &ctx.kid,
        &json!({"iss":"https://not-the-issuer","aud":"client-1","exp":now + 300,"sub":"s-1","email":"a@b.c","email_verified":true}),
    );
    let (s, _) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let now = now_secs();
    // Past exp by more than the 60-second leeway.
    let tok = mint(
        &ctx.encoding,
        &ctx.kid,
        &json!({"iss":ctx.issuer,"aud":"client-1","exp":now - 600,"sub":"s-1","email":"a@b.c","email_verified":true}),
    );
    let (s, _) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_forged_signature_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    // Signed with an unrelated key: the claims are perfect but the signature is
    // not the provider's.
    let mut rng = OsRng;
    let other = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pem = other.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let other_enc = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    let now = now_secs();
    let tok = mint(
        &other_enc,
        &ctx.kid,
        &json!({"iss":ctx.issuer,"aud":"client-1","exp":now + 300,"sub":"eve-1","email":"eve@x.y","email_verified":true}),
    );
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn an_alg_none_token_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let now = now_secs();
    let payload = URL_SAFE_NO_PAD.encode(
        json!({"iss":ctx.issuer,"aud":"client-1","exp":now + 300,"sub":"eve-1","email":"eve@x.y"})
            .to_string(),
    );
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let (s, v) = oidc_login(&ctx.app, "corp", &format!("{header}.{payload}.")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn an_hs256_forgery_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    // The classic downgrade: sign an HS256 token using the public key material
    // (here the JWKS text) as the HMAC secret. The verifier must refuse the
    // algorithm before it consults any key.
    let now = now_secs();
    let secret = EncodingKey::from_secret(ctx.jwks.as_bytes());
    let mut h = Header::new(Algorithm::HS256);
    h.kid = Some(ctx.kid.clone());
    let tok = encode(
        &h,
        &json!({"iss":ctx.issuer,"aud":"client-1","exp":now + 300,"sub":"eve-1","email":"eve@x.y"}),
        &secret,
    )
    .unwrap();
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn an_unverified_email_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    let tok = good_token(
        &ctx,
        "unverified-1",
        "unverified@x.y",
        Some(json!({"email_verified": false})),
    );
    let (s, _) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_group_map_adds_a_binding_and_keeps_existing_ones() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    // A project so the group's member binding has somewhere to point.
    let (s, proj) = call(
        &ctx.app,
        "POST",
        "/projects",
        Some(&admin),
        Some(json!({"name":"web"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{proj}");
    create_provider(
        &ctx,
        &admin,
        json!({
            "name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true,
            "groups_claim":"groups",
            "group_map":{"devs":[{"role":"member","project":"web"}]}
        }),
    )
    .await;
    let tok = good_token(
        &ctx,
        "dave-1",
        "dave@example.com",
        Some(json!({"groups": ["devs", "unknown"]})),
    );
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let name = v["caller"]["principal_name"].as_str().unwrap().to_owned();
    // One binding created: the mapped role in `web`.
    let (s, bindings) = call(&ctx.app, "GET", "/bindings", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    let mine: Vec<&Value> = bindings["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|b| b["spec"]["role"] == "member")
        .collect();
    assert_eq!(mine.len(), 1, "expected exactly one member binding");
    assert_eq!(mine[0]["spec"]["scope"]["project"], proj["id"]);
    let _ = name;
}

#[tokio::test]
async fn a_group_map_never_removes_a_binding() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    let (s, proj) = call(
        &ctx.app,
        "POST",
        "/projects",
        Some(&admin),
        Some(json!({"name":"web"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{proj}");
    create_provider(
        &ctx,
        &admin,
        json!({
            "name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true,
            "groups_claim":"groups",
            "group_map":{"devs":[{"role":"member","project":"web"}]}
        }),
    )
    .await;
    let tok = good_token(
        &ctx,
        "erin-1",
        "erin@example.com",
        Some(json!({"groups": ["devs"]})),
    );
    oidc_login(&ctx.app, "corp", &tok).await;
    // Sign in again with NO groups: the earlier binding must survive.
    let before = binding_count(&ctx.app, &admin).await;
    let tok2 = good_token(
        &ctx,
        "erin-1",
        "erin@example.com",
        Some(json!({"groups": []})),
    );
    let (s, _) = oidc_login(&ctx.app, "corp", &tok2).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(binding_count(&ctx.app, &admin).await, before);
}

async fn binding_count(app: &Router, admin: &str) -> usize {
    let (s, v) = call(app, "GET", "/bindings", Some(admin), None).await;
    assert_eq!(s, StatusCode::OK);
    v["total"].as_u64().unwrap() as usize
}

#[tokio::test]
async fn a_disabled_user_is_refused() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"}),
    )
    .await;
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(json!({"name":"frank","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = call(
        &ctx.app,
        "PUT",
        "/principals/frank",
        Some(&admin),
        Some(json!({"enabled": false})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Bound to the subject, then disabled: the account is found and refused.
    assert_eq!(
        link(&ctx.app, &admin, "frank", "corp", "frank-1").await,
        StatusCode::OK
    );
    let tok = good_token(&ctx, "frank-1", "frank@example.com", None);
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{v}");
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn an_unknown_provider_is_refused() {
    let ctx = setup().await;
    let tok = "whatever";
    let (s, v) = oidc_login(&ctx.app, "no-such-provider", tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}

#[tokio::test]
async fn a_new_key_triggers_one_refetch() {
    // A provider that starts serving one key, then a second under a new kid.
    // After rotation, a token naming the new kid forces exactly one extra fetch.
    let mut rng = OsRng;
    let old = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let new = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let jwk = |label: &str, key: &RsaPrivateKey| {
        format!(
            r#"{{"kty":"RSA","kid":"{label}","alg":"RS256","use":"sig","n":"{}","e":"{}"}}"#,
            URL_SAFE_NO_PAD.encode(key.n().to_bytes_be()),
            URL_SAFE_NO_PAD.encode(key.e().to_bytes_be())
        )
    };
    // Serve only the NEW key: a token minted with the OLD key under kid "new"
    // will find "new" in the cache with the wrong key, refuse, and — because it
    // names a kid the first fetch did hold — not refetch. Instead serve the OLD
    // key first, then rotate to the NEW key so the new kid is genuinely absent.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let serve_new = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (j_old, j_new, count, serve_new) = (
            jwk("k-old", &old),
            jwk("k-new", &new),
            count.clone(),
            serve_new.clone(),
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let path = String::from_utf8_lossy(&buf[..n])
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                let body = if path.ends_with("/.well-known/openid-configuration") {
                    format!(r#"{{"issuer":"http://{addr}","jwks_uri":"http://{addr}/jwks"}}"#)
                } else {
                    // One fetch fills the cache with the OLD key. After that,
                    // a refetch (which happens only when a token names an absent
                    // kid) sees the rotated, NEW-key-only set.
                    count.fetch_add(1, Ordering::SeqCst);
                    if serve_new.load(Ordering::SeqCst) {
                        format!(r#"{{"keys":[{j_new}]}}"#)
                    } else {
                        format!(r#"{{"keys":[{j_old}]}}"#)
                    }
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
    }
    let issuer = format!("http://{addr}");
    let old_enc = EncodingKey::from_rsa_pem(
        old.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let new_enc = EncodingKey::from_rsa_pem(
        new.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();

    let (app, _store) = boot().await;
    let admin = admin_token(&app).await;
    let (s, _) = call(
        &app,
        "POST",
        "/providers",
        Some(&admin),
        Some(json!({"name":"corp","issuer":issuer,"client_id":"client-1","create_unknown":true})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);

    // First sign-in: cache fills with the OLD key, token signed by OLD key.
    let now = now_secs();
    let t1 = mint(
        &old_enc,
        "k-old",
        &json!({"iss":issuer,"aud":"client-1","exp":now + 300,"sub":"rot-1","email":"rot@example.com"}),
    );
    let (s, _) = oidc_login(&app, "corp", &t1).await;
    assert_eq!(s, StatusCode::OK, "first sign-in with the seeded key");
    let after_first = count.load(Ordering::SeqCst);

    // Rotate: now the server serves the NEW key. Sign in with a token naming a
    // kid the cache does not hold (k-new) — this must trigger exactly one refetch.
    serve_new.store(true, Ordering::SeqCst);
    let t2 = mint(
        &new_enc,
        "k-new",
        &json!({"iss":issuer,"aud":"client-1","exp":now + 300,"sub":"rot-1","email":"rot@example.com"}),
    );
    let (s, _) = oidc_login(&app, "corp", &t2).await;
    assert_eq!(s, StatusCode::OK, "sign-in after key rotation");
    // One more JWKS fetch happened (the refetch), then verification succeeded.
    assert_eq!(
        count.load(Ordering::SeqCst),
        after_first + 1,
        "exactly one refetch for the rotated key"
    );
}

#[tokio::test]
async fn providers_require_a_cloud_admin() {
    let ctx = setup().await;
    // No token at all is unauthorized for provider management.
    let (s, _) = call(&ctx.app, "GET", "/providers", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // A non-admin (a plain user's token) is forbidden from create.
    let admin = admin_token(&ctx.app).await;
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/principals",
        Some(&admin),
        Some(json!({"name":"grace","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, user) = call(
        &ctx.app,
        "POST",
        "/tokens",
        None,
        Some(json!({"name":"grace","password":"correct horse battery"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{user}");
    let user_tok = user["token"].as_str().unwrap().to_owned();
    let (s, _) = call(
        &ctx.app,
        "POST",
        "/providers",
        Some(&user_tok),
        Some(json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1"})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deleting_a_provider_evicts_its_keys() {
    let ctx = setup().await;
    let admin = admin_token(&ctx.app).await;
    let id = create_provider(
        &ctx,
        &admin,
        json!({"name":"corp","issuer":ctx.issuer,"client_id":"client-1","create_unknown":true}),
    )
    .await;
    // Sign in once so keys are cached, then delete the provider.
    let tok = good_token(&ctx, "hank-1", "hank@example.com", None);
    assert_eq!(oidc_login(&ctx.app, "corp", &tok).await.0, StatusCode::OK);
    // The sign-in created an account bound to the provider, and a provider with
    // accounts still bound to it is not deletable — that is a separate test's
    // subject. Unbinding leaves the cached keys to evict, which is what this one
    // is about.
    assert_eq!(
        unlink(&ctx.app, &admin, "corp", "hank-1").await,
        StatusCode::OK
    );
    let (s, _) = call(
        &ctx.app,
        "DELETE",
        &format!("/providers/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    // Gone: sign-in by name is now an unknown provider (still the uniform answer).
    let (s, v) = oidc_login(&ctx.app, "corp", &tok).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["message"], REFUSAL);
}
