function hostRows(table, hosts) {
  [...table.querySelectorAll("tbody tr")].forEach((tr, idx) => {
    const h = hosts[idx];
    if (!h) return;
    if (h.spec && h.spec.disabled) {
      const p = pill("disabled");
      p.title = h.spec.disabled_reason || "disabled by an admin";
      tr.children[1].append(document.createTextNode(" "), p);
    }
    const cpu = vcpuUse(h);
    const mem = gibUse(h);
    cellText(tr, 3, cpu.join(" / "));
    tr.children[3].prepend(meter(cpu[0], cpu[1]), document.createTextNode(" "));
    cellText(tr, 4, mem.join(" / "));
    tr.children[4].prepend(meter(mem[0], mem[1]), document.createTextNode(" "));
    const cert = tr.children[6];
    if (cert.textContent && certSoon(h)) {
      cert.classList.add("warn");
      cert.title = "The agent certificate on this host expires within two weeks";
    }
    const open = () => go("hosts", false, h.id);
    nameLink(tr, 0, h.name, open);
    tr.style.cursor = "pointer";
    // The row opens the host. A button of its own stops the click here: the
    // name cell has no nesting of its own to give the click back to.
    tr.addEventListener("click", open);
    tr.querySelectorAll("button").forEach((b) => b.addEventListener("click", (ev) => ev.stopPropagation()));
  });
}

