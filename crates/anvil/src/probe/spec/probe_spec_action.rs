// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::spec::probe_action::ProbeAction;
use crate::probe::spec::probe_scheme::ProbeScheme;
use crate::probe::spec::probe_spec::ProbeSpec;

impl ProbeSpec {
    pub(in crate::probe) fn action(&self) -> ProbeAction {
        if let Some(http) = &self.http_get {
            return ProbeAction::Http {
                path: http.path.clone(),
                port: http.port,
                host: http.host.clone(),
                https: matches!(http.scheme, Some(ProbeScheme::Https)),
            };
        }
        if let Some(tcp) = &self.tcp_socket {
            return ProbeAction::Tcp { port: tcp.port };
        }
        if let Some(exec) = &self.exec {
            return ProbeAction::Exec {
                command: exec.command.clone(),
            };
        }
        ProbeAction::Missing
    }
}
