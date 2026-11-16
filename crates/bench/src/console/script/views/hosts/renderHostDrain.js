    const drain = el("button", "Drain"); drain.type = "button";
    drain.addEventListener("click", async () => {
      if (!confirm("Drain " + host.name + "? The host is disabled and what can be live-migrated is moved off it. Nothing is stopped.")) return;
      setErr("");
      drain.disabled = true;
      await report("Draining " + host.name, null, "The cloud is answering.");
      let rep = null;
      try { rep = await api("POST", "/compute/hosts/" + hid + "/drain", {}); }
      catch (e) { reportHold.delete(String(host.id)); setErr(e.message); drain.disabled = false; return; }
      await report("Drain of " + host.name, (rep.migrating || []).map(m => ({
        name: m.name, why: "live-migrating to " + (m.to_name || shortId(m.to)),
      })).concat((rep.not_moved || []).map(l => ({ name: l.name, why: "left: " + shortIds(l.reason) }))),
        "Nothing was placed on this host.");
    });
