    // The answer is held, then the page is read again: an action ends with
    // the host as the server now has it, and the report still under it.
    const report = (title, rows, empty) => {
      reportHold.set(String(host.id), { title, rows, empty });
      return render();
    };
    const toggle = el("button", spec.disabled ? "Enable" : "Disable", spec.disabled ? "primary" : "");
    toggle.type = "button";
    toggle.addEventListener("click", async () => {
      setErr("");
      toggle.disabled = true;
      try {
        if (spec.disabled) {
          await api("POST", "/compute/hosts/" + hid + "/enable");
        } else {
          const why = prompt("Why is " + host.name + " being disabled? Nothing new is placed on it. Empty for no reason.");
          if (why === null) { toggle.disabled = false; return; }
          await api("POST", "/compute/hosts/" + hid + "/disable", { reason: why });
        }
      } catch (e) { setErr(e.message); toggle.disabled = false; return; }
      await render();
    });
