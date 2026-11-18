// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::cron::Schedule;
use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;

impl Api for CronJobSpec {
    type Status = CronJobStatus;
    const NOUN: &'static str = "cronjobs";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, _current: Option<&CronJob>) -> ApiResult<()> {
        Schedule::parse(&self.schedule).map_err(|e| ApiError::bad_request(e.to_string()))?;
        self.job_template.validate(None)
    }
}
