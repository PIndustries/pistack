// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::default_sg::DEFAULT_SG;
use crate::client::functions::*;
use crate::client::Client;
use crate::types::*;
use pistack_store::StoreError;
use pistack_types::{kind, ApiResult, Id, Meta};
impl Client {
    /// The project's "default" security group, cloned from the template on
    /// first use.
    pub fn default_security_group(&self, project: &Id) -> ApiResult<SecurityGroup> {
        match self
            .security_groups()
            .get_by_name(Some(project), DEFAULT_SG)
        {
            Ok(sg) => return Ok(sg),
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(store_err(e)),
        }
        let template = self.ensure_default_template()?;
        let mut meta = Meta::new(kind::SECGROUP, DEFAULT_SG, Some(project.clone()));
        meta.description = "Default security group".into();
        meta.labels = template.meta.labels.clone();
        let me = meta.id.clone();
        let rules = template
            .spec
            .rules
            .iter()
            .map(|r| Rule {
                id: uuid::Uuid::now_v7(),
                remote: match &r.remote {
                    Remote::Group(g) if *g == template.meta.id => Remote::Group(me.clone()),
                    other => other.clone(),
                },
                ..r.clone()
            })
            .collect();
        match self.security_groups().create(SecurityGroup {
            meta,
            spec: SecurityGroupSpec { rules },
            status: SecurityGroupStatus::default(),
        }) {
            Ok(sg) => Ok(sg),
            Err(StoreError::Conflict(_)) => self
                .security_groups()
                .get_by_name(Some(project), DEFAULT_SG)
                .map_err(store_err),
            Err(e) => Err(store_err(e)),
        }
    }
}
