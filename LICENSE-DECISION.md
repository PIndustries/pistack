# Licence decision

Status: **decided 2026-10-02: Apache-2.0** for the product and all crates, DCO sign-off on every commit, PiStack name and logo held as trademarks by P. Industries LLC. The commercial layer (native Apple apps, support plans) is separate and proprietary; the core never depends on it. Nothing here is legal advice; have a lawyer confirm before the repo goes public.

The options considered are kept below for the record.

## What we want to protect

Pick the threats that matter; the licence follows from them.

| Threat | Licence that addresses it |
| --- | --- |
| Someone forks PiStack, closes it, sells it as their product | GPL-3.0-or-later, AGPL-3.0 |
| A hosting company runs PiStack as a paid service without contributing | AGPL-3.0 only |
| P. Industries wants to sell a commercial edition later | any licence + a CLA so copyright is assignable (dual-licensing) |
| Vendors and enterprises adopt and port drivers without legal friction | Apache-2.0 (also MIT, MPL-2.0) |
| Patent claims from contributors | Apache-2.0 and GPL-3.0 carry explicit patent grants; GPL-2.0 and MIT do not |

## Constraints from the code

- Most of the Rust ecosystem is MIT or dual MIT/Apache-2.0; several crates we want are **Apache-2.0 only**: youki, pingora, kube-rs, cedar-policy, opentelemetry, cryptoki, rustfs.
- OpenStack is Apache-2.0. Porting logic or reusing JSON schemas from it requires an Apache-compatible licence.
- The official libvirt binding (`virt`) is LGPL-2.1-or-later (fine for dynamic linking under any option).
- **GPL-2.0-only (the Linux kernel's licence) is incompatible with Apache-2.0** per the FSF, so choosing it means replacing every Apache-only dependency and clean-rooming everything ported from OpenStack.
- Garage is AGPL-3.0 and stays out of the tree regardless.

## Options on the table

1. **Apache-2.0** — same as OpenStack and Kubernetes. Lowest friction, biggest ecosystem pull, explicit patent grant. No copyleft. Pair with a trademark note so "PiStack" stays P. Industries' name even if the code is forked.
2. **GPL-3.0-or-later** — kernel-style copyleft that is Apache-compatible. Blocks closed forks; does not cover SaaS hosting. Some enterprises refuse GPLv3 in shipped products.
3. **AGPL-3.0 + CLA** — the open-core playbook (Grafana, MinIO, Garage). Covers SaaS hosting and enables commercial dual-licensing. Enterprises are twitchy about AGPL.
4. **MPL-2.0** — file-level copyleft, permissive linking. Middle ground; less common in infra.

Recommendation on the table: Apache-2.0 for the product and MIT OR Apache-2.0 for the shared crates, with DCO sign-off on every commit and a trademark note. Switch to AGPL+CLA only if the SaaS-hosting threat is a real business concern.

## Either way

- DCO sign-off (`git commit -s`) on every commit, enforced in CI.
- `SPDX-License-Identifier` header on every source file once decided.
- Trademark: the PiStack name and logo belong to P. Industries LLC.
