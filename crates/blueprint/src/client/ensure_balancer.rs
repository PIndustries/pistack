// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};

use super::found::found;
use super::listener_doc_type::ListenerDoc;
use super::project_allows::project_allows;

pub(crate) fn ensure_balancer(
    relay: &relay::Client,
    store: &Store,
    project: Option<&Id>,
    listener: &str,
    doc: &ListenerDoc,
) -> ApiResult<relay::Balancer> {
    let name = {
        let given = doc.balancer.trim();
        if given.is_empty() {
            listener.to_owned()
        } else {
            given.to_owned()
        }
    };
    if let Some(id) = Id::parse(kind::BALANCER, &name) {
        let balancer = store
            .collection::<relay::Balancer>()
            .get(&id)
            .map_err(ApiError::from)?;
        if !project_allows(project, balancer.meta.project.as_ref()) {
            return Err(ApiError::bad_request(format!(
                "listener {listener}: balancer '{name}' is not in this project"
            )));
        }
        return Ok(balancer);
    }
    if let Some(existing) = found::<relay::Balancer>(store, project, &name)? {
        return Ok(existing);
    }
    let vip = doc.vip_network.trim();
    if vip.is_empty() {
        return Err(ApiError::bad_request(format!(
            "listener {listener}: spec.vip_network is required to create balancer '{name}'"
        )));
    }
    let mut spec = relay::BalancerSpec {
        vip_network: vip.to_owned(),
        ..relay::BalancerSpec::default()
    };
    if let Some(provider) = doc.provider {
        spec.provider = provider;
    }
    relay
        .create_balancer(Meta::new(kind::BALANCER, name, project.cloned()), spec)
        .map_err(|e| ApiError::new(e.code, format!("listener {listener}: {}", e.message)))
}
