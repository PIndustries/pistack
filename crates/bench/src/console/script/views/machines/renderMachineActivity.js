    activity() {
      const wrap = el("div", null, "panel");
      if (auditPage === null) wrap.append(el("p", "The record of who did what is kept for cloud admins."));
      else {
        const seen = new Set();
        const records = [...itemsOf(auditPage), ...(auditById ? itemsOf(auditById) : [])]
          .filter(r => !seen.has(r.id) && seen.add(r.id))
          .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
        wrap.append(activityFeed(records, "Nothing is recorded for this machine."));
      }
      pane.append(wrap);
    },
  };
  const names = { details: "Details", networking: "Networking", volumes: "Volumes (" + vols.length + ")", console: "Console", activity: "Activity" };
  if (!panes[machineTab]) machineTab = "details";
  const select = (key) => {
    machineTab = key;
    for (const b of tabs.children) b.setAttribute("aria-selected", String(b.dataset.tab === key));
    pane.replaceChildren();
    panes[key]();
  };
  for (const key of Object.keys(panes)) {
    const b = el("button", names[key]); b.type = "button";
    b.dataset.tab = key;
    b.setAttribute("role", "tab");
    b.addEventListener("click", () => select(key));
    tabs.append(b);
  }
  root.append(tabs, pane);
  select(machineTab);

