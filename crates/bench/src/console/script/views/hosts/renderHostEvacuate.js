    // Only a host that has stopped reporting, or one an operator disabled
    // and says is dead. Anything else risks a second copy of a running
    // machine, and the server says so; the button does not pretend otherwise.
    const evacuate = el("button", "Evacuate", "danger"); evacuate.type = "button";
    evacuate.disabled = !spec.disabled && phase === "ready";
    evacuate.title = evacuate.disabled
      ? "Evacuate a host that has stopped reporting, or one you disabled and know is dead"
      : "Hand the machines of a host that is gone back to the scheduler";
    evacuate.addEventListener("click", () => {
      const box = el("div", null, "report");
      box.append(el("h3", "Evacuate " + host.name));
      box.append(el("p", "The machines are handed back to the scheduler and start again elsewhere. A root disk is a file on this host: it does not come with them. Volumes on shared storage are plugged back in.", "sub"));
      const dead = el("input"); dead.type = "checkbox";
      const rebuild = el("input"); rebuild.type = "checkbox";
      const tick = (input, id, label, why) => {
        input.id = id;
        input.title = why;
        const row = el("label", null, "checkrow");
        row.append(input, el("span", label));
        return row;
      };
      box.append(
        tick(dead, "ev-dead", "This host is dead",
          "Needed when the host is disabled but still reporting to the cloud"),
        tick(rebuild, "ev-rebuild", "Start each machine again from its image, losing its root disk",
          "A machine that cannot be started this way is reported and left where it is"),
      );
      const run = el("button", "Evacuate", "danger"); run.type = "button";
      const cancel = el("button", "Cancel"); cancel.type = "button";
      const tools = el("p", null, "tools");
      tools.append(run, cancel);
      box.append(tools);
      // Without the rebuild the server evacuates nothing: every machine is
      // skipped, its root disk left behind. So the rebuild box is what
      // unlocks. "This host is dead" is a confirmation the server may ask
      // for, never a way in on its own.
      run.disabled = true;
      rebuild.addEventListener("change", () => { run.disabled = !rebuild.checked; });
      dead.addEventListener("change", () => { run.disabled = !rebuild.checked; });
      run.addEventListener("click", async () => {
        setErr("");
        run.disabled = true;
        await report("Evacuating " + host.name, null, "The cloud is answering.");
        let rep = null;
        try {
          rep = await api("POST", "/compute/hosts/" + hid + "/evacuate",
            { confirm_dead: dead.checked, rebuild_from_image: rebuild.checked });
        } catch (e) { reportHold.delete(String(host.id)); setErr(e.message); run.disabled = false; return; }
        await report("Evacuate of " + host.name, (rep.evacuated || []).map(v => ({
          name: v.name,
          why: "starts again from " + v.rebuilt_from + ((v.volumes || []).length ? " and keeps " + v.volumes.join(", ") : ""),
        })).concat((rep.skipped || []).map(l => ({ name: l.name, why: "skipped: " + shortIds(l.reason) }))),
          "No machine was recorded on this host.");
      });
      cancel.addEventListener("click", () => box.remove());
      root.append(box);
    });
    acts.append(toggle, drain, evacuate);
