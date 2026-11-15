// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet::PodNetwork;
use crate::runtime::youki::bundles::privatize_bundle;
use crate::runtime::youki::bundles::read_state;
use crate::runtime::youki::bundles::remove_bundle;
use crate::runtime::youki::bundles::write_state;
use crate::runtime::youki::exec::exec_detach;
use crate::runtime::youki::exec::exec_youki;
use crate::runtime::youki::exec::user_ns_join;
use crate::runtime::youki::lifecycle::container_missing;
use crate::runtime::youki::lifecycle::create_container;
use crate::runtime::youki::lifecycle::parse_state_pid;
use crate::runtime::youki::lifecycle::pause_stdio;
use crate::runtime::youki::lifecycle::release_failed_create;
use crate::runtime::youki::lifecycle::run_youki;
use crate::runtime::youki::lifecycle::workload_stdio;
use crate::runtime::youki::network::joined_netns;
use crate::runtime::youki::network::joins_new_net;
use crate::runtime::youki::network::observe;
use crate::runtime::youki::network::running_from;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::runtime::Saved;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::volumes::container_dir;
use crate::runtime::youki::volumes::container_youki_id;
use crate::runtime::youki::volumes::ensure_slab_sources;
use crate::runtime::youki::volumes::prepare_pod;
use crate::runtime::youki::volumes::read_disks;
use crate::runtime::youki::volumes::write_container_configs;
/// Create the sandbox, then every container, joining the sandbox pid, network,
/// ipc and uts namespaces. `state.json` is written only after they are running.
use crate::runtime::MachineRequest;
use crate::runtime::PowerState;
use crate::runtime::RunningMachine;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) async fn start_pod(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    req: &MachineRequest,
    plan: &PodPlan,
    prior: bool,
    net: &PodNetwork,
) -> ApiResult<RunningMachine> {
    // Fail before youki create. A missing path, or a directory that is not
    // a mount point, would otherwise land in config.json and the pod would
    // write onto the host disk.
    let disks = read_disks(bundle)?;
    ensure_slab_sources(plan, &disks)?;
    if prior {
        if let Some(saved) = read_state(bundle).await? {
            for cid in saved.containers.iter().rev() {
                if let Err(e) = exec_youki(bin, mode, &["delete", cid], None).await {
                    if !container_missing(&e) {
                        return Err(e);
                    }
                }
            }
        }
    }
    let limits = Limits {
        vcpus: req.resources.vcpus,
        memory_mib: req.resources.memory_mib,
    };
    let hostname = if req.name.is_empty() {
        req.id.to_string()
    } else {
        req.name.clone()
    };
    let id = req.id.clone();
    let observed = observe(req, net);
    let bundle_prep = bundle.to_path_buf();
    let plan_prep = plan.clone();
    let host_prep = hostname;
    let image = req.image.clone();
    let mode_prep = mode.clone();
    let host_netns = joined_netns(net);
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_pod(
            &bundle_prep,
            &host_prep,
            &limits,
            &plan_prep,
            &image,
            &mode_prep,
            host_netns.as_deref(),
        )
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?;
    let processes = match prepared {
        Ok(processes) => processes,
        Err(e) => {
            if !prior {
                let _ = remove_bundle(bundle, &mode.privilege).await;
            }
            return Err(e);
        }
    };
    let id_str = id.to_string();
    let pause = pause_stdio(bundle);
    // A stopped pod's namespace is gone. Create the sandbox again so it
    // joins the one `provision_network` just made. `youki start` alone
    // would keep the empty namespace from the first create.
    let recreate = joins_new_net(net);
    if !prior || recreate {
        let wipe = if prior { None } else { Some(bundle) };
        create_container(bin, mode, bundle, &id_str, None, &pause, wipe).await?;
        if let Err(e) = exec_detach(bin, mode, &["start", &id_str], None, &pause).await {
            release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
            return Err(e);
        }
    } else {
        exec_detach(bin, mode, &["start", &id_str], None, &pause).await?;
    }
    let state = match run_youki(bin, mode, &["state", &id_str], None).await {
        Ok(output) => output,
        Err(e) => {
            if !prior || recreate {
                release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
            }
            return Err(e);
        }
    };
    let Some(pid) = parse_state_pid(&state.stdout) else {
        if !prior || recreate {
            release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
        }
        return Err(ApiError::unavailable(
            "youki runtime: sandbox state has no pid",
        ));
    };
    let bundle_cfg = bundle.to_path_buf();
    let plan_cfg = plan.clone();
    let limits_cfg = limits;
    let mode_cfg = mode.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        let disks = read_disks(&bundle_cfg)?;
        write_container_configs(
            &bundle_cfg,
            &plan_cfg,
            &processes,
            &disks,
            &limits_cfg,
            pid,
            &mode_cfg,
        )
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?
    {
        if !prior || recreate {
            release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
        }
        return Err(e);
    }
    let user_ns = user_ns_join(mode, pid);
    let mut started = Vec::new();
    for index in 0..plan.containers.len() {
        let cid = container_youki_id(&id, index);
        let cpath = container_dir(bundle, index);
        let io = workload_stdio(&cpath);
        if let Err(e) =
            create_container(bin, mode, &cpath, &cid, user_ns.as_deref(), &io, None).await
        {
            if !prior || recreate {
                release_failed_create(bin, mode, bundle, &id_str, &started, prior).await;
            }
            return Err(e);
        }
        if let Err(e) = exec_detach(bin, mode, &["start", &cid], user_ns.as_deref(), &io).await {
            if !prior || recreate {
                let _ = exec_youki(bin, mode, &["delete", "--force", &cid], None).await;
                release_failed_create(bin, mode, bundle, &id_str, &started, prior).await;
            }
            return Err(e);
        }
        started.push(cid);
    }
    let _ = privatize_bundle(bundle, &mode.privilege);
    write_state(
        bundle,
        &Saved {
            id: id_str,
            addresses: observed.addresses.clone(),
            power: PowerState::On,
            containers: started,
            container_names: plan.containers.iter().map(|c| c.name.clone()).collect(),
            probes: crate::probe::probes_in_user_data(req.user_data.as_deref()),
            netns: observed.netns.clone(),
            veths: observed.veths.clone(),
            ovs: observed.ovs.clone(),
            network_message: observed.message.clone(),
        },
    )
    .await?;
    Ok(running_from(id, &observed, PowerState::On))
}
