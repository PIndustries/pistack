// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::{Duration, Instant};
use uuid::Uuid;

use super::*;

#[test]
fn ticket_expiry_and_wrong_machine() {
    let ticket = Uuid::now_v7().simple().to_string();
    assert_eq!(ticket.len(), 32);
    assert!(ticket.chars().all(|c| c.is_ascii_hexdigit()));
    {
        let mut map = TICKETS.lock();
        map.insert(
            ticket.clone(),
            Ticket {
                machine_id: "mach-a".into(),
                expires: Instant::now() + TICKET_TTL,
            },
        );
    }
    assert!(check_ticket(&ticket, "mach-b").is_err());
    assert!(check_ticket(&ticket, "mach-a").is_ok());

    let expired = format!("{ticket}ee");
    {
        let mut map = TICKETS.lock();
        map.insert(
            expired.clone(),
            Ticket {
                machine_id: "mach-a".into(),
                expires: Instant::now() - Duration::from_secs(5),
            },
        );
    }
    assert!(check_ticket(&expired, "mach-a").is_err());
    assert!(take_ticket(&expired, "mach-a").is_err());
    assert!(check_ticket(&ticket, "mach-a").is_ok());
    assert!(take_ticket(&ticket, "mach-a").is_ok());
    assert!(take_ticket(&ticket, "mach-a").is_err());
    assert!(query_ticket(None).is_err());
    assert!(query_ticket(Some("")).is_err());
    assert!(query_ticket(Some("ticket=")).is_err());
    assert_eq!(query_ticket(Some("other=1&ticket=abc")).unwrap(), "abc");
}

#[test]
fn vnc_port_accepts_only_loopback() {
    assert_eq!(vnc_port(":1").unwrap(), 5901);
    assert_eq!(vnc_port("127.0.0.1:2").unwrap(), 5902);
    assert_eq!(vnc_port("127.0.0.1:0\n").unwrap(), 5900);
    assert_eq!(vnc_port("localhost:3").unwrap(), 5903);
    assert_eq!(vnc_port("::1:4").unwrap(), 5904);
    assert_eq!(vnc_port("[::1]:5").unwrap(), 5905);
    assert!(vnc_port("10.66.66.1:1").is_err());
    assert!(vnc_port("0.0.0.0:0").is_err());
    assert!(vnc_port("").is_err());
    assert!(vnc_port("   ").is_err());
    assert!(vnc_port(":").is_err());
    assert!(domain_name("mach-01a1007f-5708-7534-8fdf-a9bec59c383b").is_ok());
    assert!(domain_name("mach-1;rm").is_err());
    assert!(domain_name("../etc").is_err());
    assert!(domain_name("").is_err());
}
