// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The five handlers every kind shares. Expanded by `router`, where the
//! generated code resolves its names.

macro_rules! crud {
    (
        $ty:ty, $spec:ty, $kind:expr, $col:ident, $filter:expr,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident $(, $await:tt)?
    ) => {
        async fn $list(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Query(q): Query<ListQuery>,
        ) -> ApiResult<Json<Page<$ty>>> {
            let project = scope(&caller, &sq)?;
            let mut items = c.$col().list(project.as_ref())?;
            let filter: fn(&Client, Option<&Id>, &ScopeQuery, &mut Vec<$ty>) -> ApiResult<()> = $filter;
            filter(&c, project.as_ref(), &sq, &mut items)?;
            Ok(Json(page(items, &q)))
        }

        pub(super) async fn $create(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Json(body): Json<Create<$spec>>,
        ) -> ApiResult<Response> {
            let project = create_project(&caller, &body.project)?;
            let meta = new_meta($kind, &body, project)?;
            Ok(created(c.$c_create(meta, body.spec)?))
        }

        pub(super) async fn $get(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<Json<$ty>> {
            let project = scope(&caller, &sq)?;
            Ok(Json(visible(
                &caller,
                c.$col().resolve(project.as_ref(), &id)?,
            )?))
        }

        pub(super) async fn $update(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
            headers: HeaderMap,
            Json(body): Json<Update<$spec>>,
        ) -> ApiResult<Json<$ty>> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            let rev = if_match(&headers)?;
            Ok(Json(c.$c_update(&cur.meta.id, rev, |r| {
                apply_meta(&mut r.meta, &body);
                r.spec = body.spec.clone();
                Ok(())
            })?))
        }

        pub(super) async fn $delete(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<StatusCode> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            c.$c_delete(&cur.meta.id)$(.$await)??;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}
