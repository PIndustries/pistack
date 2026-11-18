// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/network`.
//!
//! Plain nouns, one shape for every kind:
//! `GET|POST /{nouns}`, `GET|PUT|DELETE /{nouns}/{name-or-id}`, plus the
//! few verbs that are not a spec edit (router interfaces, security group
//! rules, floating IP association). Handlers only validate the caller's
//! scope and hand off to `Client`; they never write `status`.
//!
//! The vpc has the same shape under `/vpcs`, and one shortcut:
//! `POST /vpcs/{id}/subnets` makes a network and its one subnet together.

macro_rules! crud {
    (
        $ty:ty, $spec:ty, $kind:expr, $col:ident,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident
    ) => {
        crud!(
            $ty, $spec, $kind, $col, $list, $create, $get, $update, $delete, $c_create, $c_update,
            $c_delete, no_vpc
        );
    };
    (
        $ty:ty, $spec:ty, $kind:expr, $col:ident,
        $list:ident, $create:ident, $get:ident, $update:ident, $delete:ident,
        $c_create:ident, $c_update:ident, $c_delete:ident,
        $net_of:ident
    ) => {
        pub(crate) async fn $list(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Query(q): Query<ListQuery>,
            Query(f): Query<NetFilter>,
        ) -> ApiResult<Json<Page<$ty>>> {
            let project = scope(&caller, &sq)?;
            let want = InWhich::parse(&c, &caller, project.as_ref(), &f.vpc)?;
            let items = c.$col().list(project.as_ref())?;
            Ok(Json(page_by(items, &q, |x| {
                want.holds($net_of(&c, x).as_ref())
            })))
        }

        pub(crate) async fn $create(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Json(body): Json<Create<$spec>>,
        ) -> ApiResult<Response> {
            let project = create_project(&caller, &body.project)?;
            body.spec.guard(&caller, None)?;
            let meta = new_meta($kind, &body, project)?;
            Ok(created(c.$c_create(meta, body.spec)?))
        }

        pub(crate) async fn $get(
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

        pub(crate) async fn $update(
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
            body.spec.guard(&caller, Some(&cur.spec))?;
            Ok(Json(c.$c_update(&cur.meta.id, rev, |r| {
                apply_meta(&mut r.meta, &body);
                r.spec = body.spec.clone();
                Ok(())
            })?))
        }

        pub(crate) async fn $delete(
            State(c): State<Client>,
            Extension(caller): Extension<Caller>,
            Query(sq): Query<ScopeQuery>,
            Path(id): Path<String>,
        ) -> ApiResult<StatusCode> {
            let project = scope(&caller, &sq)?;
            let cur = visible(&caller, c.$col().resolve(project.as_ref(), &id)?)?;
            c.$c_delete(&cur.meta.id)?;
            Ok(StatusCode::NO_CONTENT)
        }
    };
}

mod add_rule;
mod apply_meta;
mod associate;
mod associate_fip;
mod create;
mod create_project;
mod create_vpc;
mod create_vpc_subnet;
mod created;
mod crud_deps;
mod crud_generated;
mod default_dhcp;
mod default_limit;
mod delete_vpc;
mod delete_vpc_subnet;
mod disassociate_fip;
mod floating_ip;
mod floating_ip_spec;
mod get_vpc;
mod guarded;
mod if_match;
mod in_which;
mod info;
mod interfaces;
mod list_vpc_subnets;
mod list_vpcs;
mod max_limit;
mod net_filter;
mod network;
mod network_spec;
mod new_meta;
mod no_vpc;
mod own_vpc;
mod page;
mod page_by;
mod port;
mod port_spec;
mod remove_rule;
mod require_caller;
mod router;
mod router_spec;
mod routers;
mod scope;
mod scope_query;
mod security_group;
mod security_group_spec;
mod set_router_interfaces;
mod subnet;
mod subnet_spec;
mod update;
mod update_vpc;
mod visible;
mod vpc_of_subnet;
mod vpc_spec;
mod vpc_subnet;
pub use associate::Associate;
pub use create::Create;
pub use interfaces::Interfaces;
pub use net_filter::NetFilter;
pub use router::router;
pub use scope_query::ScopeQuery;
pub use update::Update;
pub use vpc_subnet::VpcSubnet;
