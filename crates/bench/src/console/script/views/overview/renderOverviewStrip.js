  // The servers that run this cloud, as the one answering sees them.
  if (cluster && Array.isArray(cluster.members)) {
    const strip = el("div", null, "strip");
    strip.append(el("span", "Control plane", "name"));
    const voters = cluster.members.filter(m => String(m.role).toLowerCase() === "voter");
    for (const m of cluster.members) {
      const srv = el("span", null, "srv");
      srv.append(String(m.api || "").replace(/^https?:\/\//, ""));
      const leads = m.id === cluster.leader_id;
      srv.append(pill(leads ? "leader" : String(m.role || "").toLowerCase()));
      srv.title = m.id;
      strip.append(srv);
    }
    const facts = [voters.length + (voters.length === 1 ? " voter" : " voters")];
    if (!cluster.leader_id) facts.unshift("no leader");
    if (cluster.commit !== undefined) facts.push("entry " + cluster.commit);
    if (info && info.version) facts.push("build " + info.version);
    strip.append(el("span", facts.join(" · "), cluster.leader_id ? "facts" : "facts bad"));
    root.append(strip);
  }

  // The last minutes, added up across hosts and machines.
