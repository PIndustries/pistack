// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::default_sg::DEFAULT_SG;
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use crate::usage::{self, released};
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn delete_security_group(&self, id: &Id) -> ApiResult<SecurityGroup> {
        let sg = self.security_groups().get(id).map_err(store_err)?;
        if sg.meta.project.is_none() && sg.meta.name == DEFAULT_SG {
            return Err(ApiError::forbidden(
                "the cloud-wide default security group template cannot be deleted",
            ));
        }
        let users = self
            .ports()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| p.spec.security_groups.contains(id))
            .count();
        if users > 0 {
            return Err(ApiError::conflict(format!(
                "security group {id} is used by {users} port(s)"
            )));
        }
        for other in self.security_groups().list(None).map_err(store_err)? {
            if other.meta.id != *id
                && other
                    .spec
                    .rules
                    .iter()
                    .any(|r| r.remote == Remote::Group(id.clone()))
            {
                return Err(ApiError::conflict(format!(
                    "security group {id} is referenced by rules of {}",
                    other.meta.id
                )));
            }
        }
        let sg = self.security_groups().delete(id).map_err(store_err)?;
        released(
            &self.store,
            sg.meta.project.as_ref(),
            usage::SECURITY_GROUPS,
        );
        Ok(sg)
    }
}
