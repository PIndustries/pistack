// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet;
use crate::runtime::youki::bundle_dir::bundle_dir;
use crate::runtime::youki::bundles::privatize_bundle;
use crate::runtime::youki::bundles::read_state;
use crate::runtime::youki::bundles::remove_bundle;
use crate::runtime::youki::bundles::write_state;
use crate::runtime::youki::ensure_state_dir::ensure_state_dir;
use crate::runtime::youki::exec::exec_detach;
use crate::runtime::youki::exec::exec_youki;
use crate::runtime::youki::lifecycle::container_missing;
use crate::runtime::youki::lifecycle::create_container;
use crate::runtime::youki::lifecycle::forget_failed_create;
use crate::runtime::youki::lifecycle::start_pod;
use crate::runtime::youki::lifecycle::workload_stdio;
use crate::runtime::youki::network::drop_link;
use crate::runtime::youki::network::ip_privilege;
use crate::runtime::youki::network::joined_netns;
use crate::runtime::youki::network::joins_new_net;
use crate::runtime::youki::network::nic_addresses;
use crate::runtime::youki::network::observe;
use crate::runtime::youki::network::provision_network;
use crate::runtime::youki::network::release_recorded;
use crate::runtime::youki::network::running_from;
use crate::runtime::youki::parse_id::parse_id;
use crate::runtime::youki::resolve_run_mode::resolve_run_mode;
use crate::runtime::youki::resolve_youki::resolve_youki;
use crate::runtime::youki::runtime::lock;
use crate::runtime::youki::runtime::Saved;
use crate::runtime::youki::runtime::YoukiRuntime;
use crate::runtime::youki::spec::sandbox_plan;
use crate::runtime::youki::spec::write_bundle;
use crate::runtime::youki::state_dir::state_dir;
use crate::runtime::youki::volumes::attach_host_path;
use crate::runtime::youki::volumes::check_device_name;
use crate::runtime::youki::volumes::host_path_ok;
use crate::runtime::youki::volumes::next_device;
use crate::runtime::youki::volumes::read_disks;
use crate::runtime::youki::volumes::refresh_disk_mounts;
use crate::runtime::youki::volumes::release_rbd;
use crate::runtime::youki::volumes::unmap_recorded;
use crate::runtime::youki::volumes::write_disks;
use crate::runtime::youki::volumes::AttachedDisk;
use crate::runtime::MachineRequest;
use crate::runtime::MachineRuntime;
use crate::runtime::PowerState;
use crate::runtime::RunningMachine;
use async_trait::async_trait;
use pistack_rpc::proto::ExecOutput;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use pistack_types::Id;
use std::time::Duration;

#[async_trait]
impl MachineRuntime for YoukiRuntime {
    fn name(&self) -> &'static str {
        "youki"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let _guard = lock().lock().await;
        let bin = resolve_youki()?;
        // Reject an unusable state directory before reading a bundle under it.
        // A path that is not a directory otherwise surfaces as a read error.
        ensure_state_dir()?;
        let id = req.id.clone();
        let bundle = bundle_dir(&id)?;
        let prior = read_state(&bundle).await?;
        let probes = crate::probe::probes_in_user_data(req.user_data.as_deref());
        if let Some(saved) = prior.as_ref() {
            if saved.power == PowerState::On && parse_id(&saved.id)? == id {
                // Already running: do not exec youki again. Addresses come
                // from this request, not from whatever was stored. Refresh
                // the probe list so a restarted agent can keep checking.
                if saved.probes != probes {
                    let mut updated = saved.clone();
                    updated.probes = probes;
                    write_state(&bundle, &updated).await?;
                }
                // A rootless refusal stays on the record. Reporting the
                // request address would say the guest has a network it does not.
                let (addresses, message) = if saved.network_message.is_empty() {
                    (nic_addresses(req), String::new())
                } else {
                    (Vec::new(), saved.network_message.clone())
                };
                return Ok(RunningMachine {
                    id,
                    addresses,
                    power: PowerState::On,
                    message,
                });
            }
        }
        let mode = resolve_run_mode()?;
        // user_data and ssh_keys are not written, logged, or passed to youki.
        // PISTACK_YOUKI_NETNS skips this and is joined as the network namespace.
        let net = match provision_network(&bundle, req, &mode).await {
            Ok(net) => net,
            Err(e) => {
                if prior.is_none() {
                    let _ = remove_bundle(&bundle, &mode.privilege).await;
                }
                return Err(e);
            }
        };
        let plan = match sandbox_plan(req) {
            Ok(plan) => plan,
            Err(e) => {
                drop_link(&mode, &net).await;
                if prior.is_none() {
                    let _ = remove_bundle(&bundle, &mode.privilege).await;
                }
                return Err(e);
            }
        };
        if let Some(plan) = plan {
            let started = start_pod(&bin, &mode, &bundle, req, &plan, prior.is_some(), &net).await;
            if started.is_err() {
                drop_link(&mode, &net).await;
            }
            return started;
        }
        let host_netns = joined_netns(&net);
        if let Err(e) = write_bundle(&bundle, req, &mode, host_netns).await {
            drop_link(&mode, &net).await;
            if prior.is_none() {
                let _ = remove_bundle(&bundle, &mode.privilege).await;
            }
            return Err(e);
        }
        let id_str = id.to_string();
        let io = workload_stdio(&bundle);
        // Stop deleted the namespace. `youki start` does not join a new one,
        // so a container that has a link is created again. A first start is too.
        let create_new = prior.is_none() || joins_new_net(&net);
        if create_new {
            let wipe = if prior.is_none() {
                Some(bundle.as_path())
            } else {
                None
            };
            if let Err(e) = create_container(&bin, &mode, &bundle, &id_str, None, &io, wipe).await {
                drop_link(&mode, &net).await;
                return Err(e);
            }
            if let Err(e) = exec_detach(&bin, &mode, &["start", &id_str], None, &io).await {
                // Create succeeded and start did not. Drop the container so a
                // retry can create it again, and do not record it as running.
                // A restart keeps the bundle; the record is still on disk.
                drop_link(&mode, &net).await;
                if prior.is_none() {
                    forget_failed_create(&bin, &mode, &id_str, Some(&bundle)).await;
                } else {
                    let _ = exec_youki(&bin, &mode, &["delete", "--force", &id_str], None).await;
                }
                return Err(e);
            }
        } else if let Err(e) = exec_detach(&bin, &mode, &["start", &id_str], None, &io).await {
            drop_link(&mode, &net).await;
            return Err(e);
        }
        // youki may have loosened the bundle directory. The agent still owns
        // it unless youki changed the owner, in which case this is a no-op.
        let _ = privatize_bundle(&bundle, &mode.privilege);
        let observed = observe(req, &net);
        if let Err(e) = write_state(
            &bundle,
            &Saved {
                id: id_str,
                addresses: observed.addresses.clone(),
                power: PowerState::On,
                containers: Vec::new(),
                container_names: Vec::new(),
                probes: Vec::new(),
                netns: observed.netns.clone(),
                veths: observed.veths.clone(),
                ovs: observed.ovs.clone(),
                network_message: observed.message.clone(),
            },
        )
        .await
        {
            drop_link(&mode, &net).await;
            return Err(e);
        }
        Ok(running_from(id, &observed, PowerState::On))
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let bin = resolve_youki()?;
        let bundle = bundle_dir(id)?;
        let Some(mut saved) = read_state(&bundle).await? else {
            return Err(ApiError::not_found(format!(
                "youki runtime: machine {id} is not on this host"
            )));
        };
        if saved.power == PowerState::Off {
            return Ok(());
        }
        let mode = resolve_run_mode()?;
        // Drop the veth and namespace before the kill, and before power is
        // recorded Off, so a failed release is retried.
        release_recorded(&mode, &saved, &bundle).await?;
        // Containers first, then the sandbox that owns their namespaces.
        let mut order: Vec<String> = saved.containers.iter().rev().cloned().collect();
        order.push(id.to_string());
        for cid in &order {
            exec_youki(&bin, &mode, &["kill", cid, "SIGTERM"], None).await?;
        }
        saved.power = PowerState::Off;
        saved.netns.clear();
        saved.veths.clear();
        saved.ovs.clear();
        saved.network_message.clear();
        write_state(&bundle, &saved).await
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let bin = resolve_youki()?;
        let bundle = bundle_dir(id)?;
        if !bundle.exists() {
            return Ok(());
        }
        let mode = resolve_run_mode()?;
        let saved = read_state(&bundle).await?;
        let had_state = saved.is_some();
        if let Some(saved) = saved.as_ref() {
            release_recorded(&mode, saved, &bundle).await?;
        } else if let Some(recorded) = podnet::read_record(&bundle) {
            podnet::release(
                &ip_privilege(&mode),
                &recorded.netns,
                &recorded.veths,
                &recorded.ovs,
            )
            .await?;
        }
        let mut order: Vec<String> = saved
            .map(|s| s.containers)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        order.push(id.to_string());
        for cid in &order {
            if let Err(e) = exec_youki(&bin, &mode, &["delete", "--force", cid], None).await {
                // A bundle with no state was never created in youki. A container
                // youki has already forgotten is gone too. Anything else stays on
                // disk so a later delete can retry.
                if had_state && !container_missing(&e) {
                    return Err(e);
                }
            }
        }
        unmap_recorded(&bundle).await?;
        remove_bundle(&bundle, &mode.privilege).await
    }

    async fn attach_volume(&self, req: &crate::runtime::VolumeAttach) -> ApiResult<String> {
        let _guard = lock().lock().await;
        let bundle = bundle_dir(&req.machine)?;
        if !bundle.join("config.json").exists() && read_state(&bundle).await?.is_none() {
            return Err(ApiError::not_found(format!(
                "youki runtime: machine {} is not on this host",
                req.machine
            )));
        }
        let (path, rbd_mapped) = attach_host_path(req).await?;
        host_path_ok(&path)?;
        let mut disks = read_disks(&bundle)?;
        let vol = req.volume.to_string();
        let device = if let Some(name) = req.device.as_deref() {
            check_device_name(name)?;
            name.to_owned()
        } else if let Some(existing) = disks.iter().find(|d| d.volume == vol) {
            existing.device.clone()
        } else {
            next_device(&disks)?
        };
        if let Some(slot) = disks.iter_mut().find(|d| d.volume == vol) {
            slot.path = path;
            slot.device = device.clone();
            slot.read_only = req.read_only;
            slot.rbd_mapped = rbd_mapped;
        } else {
            disks.push(AttachedDisk {
                volume: vol,
                path,
                device: device.clone(),
                read_only: req.read_only,
                rbd_mapped,
            });
        }
        write_disks(&bundle, &disks)?;
        refresh_disk_mounts(&bundle)?;
        Ok(device)
    }

    async fn detach_volume(&self, req: &crate::runtime::VolumeDetach) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let bundle = bundle_dir(&req.machine)?;
        if !bundle.join("config.json").exists() && read_state(&bundle).await?.is_none() {
            return Err(ApiError::not_found(format!(
                "youki runtime: machine {} is not on this host",
                req.machine
            )));
        }
        let vol = req.volume.to_string();
        let mut disks = read_disks(&bundle)?;
        let recorded = disks.iter().find(|d| d.volume == vol).cloned();
        release_rbd(req, recorded.as_ref()).await?;
        let before = disks.len();
        disks.retain(|d| d.volume != vol);
        if disks.len() != before {
            write_disks(&bundle, &disks)?;
            refresh_disk_mounts(&bundle)?;
        }
        Ok(())
    }

    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        let _guard = lock().lock().await;
        let _bin = resolve_youki()?;
        let root = state_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut rd = tokio::fs::read_dir(&root).await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
        })?;
        let mut out = Vec::new();
        while let Some(ent) = rd.next_entry().await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
        })? {
            let ft = ent.file_type().await.map_err(|e| {
                ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
            })?;
            // Skip symlinks. file_type does not follow them.
            if !ft.is_dir() {
                continue;
            }
            let Some(saved) = read_state(&ent.path()).await? else {
                continue;
            };
            out.push(RunningMachine {
                id: parse_id(&saved.id)?,
                addresses: saved.addresses,
                power: saved.power,
                message: saved.network_message,
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    async fn container_probes(&self, id: &Id) -> Vec<crate::probe::ContainerProbes> {
        let Ok(bundle) = bundle_dir(id) else {
            return Vec::new();
        };
        let _guard = lock().lock().await;
        match read_state(&bundle).await {
            Ok(Some(saved)) => saved.probes,
            _ => Vec::new(),
        }
    }

    /// Command probe. `env` is empty and `cwd` is unset, so the container
    /// keeps its own environment and working directory. Those values are
    /// not read and not logged. Privilege follows `exec`: `sudo -n` when
    /// `PISTACK_YOUKI_SUDO` is set, and the rootless fallback otherwise.
    async fn command_probe(
        &self,
        id: &Id,
        container: &str,
        argv: &[String],
        timeout: Duration,
    ) -> ApiResult<ExecOutput> {
        self.exec(
            id,
            container,
            argv,
            &[],
            None,
            timeout,
            crate::probe::PROBE_EXEC_OUTPUT_CAP,
        )
        .await
    }
}
