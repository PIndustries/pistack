function launchWizard() {
  const form = el("form");
  form.className = "create wiz";
  form.autocomplete = "off";
  const state = {
    image: new Set(), size: new Set(), networks: new Set(), groups: new Set(), zone: new Set([""]),
  };
  const name = el("input"); name.required = true; name.placeholder = "web-1"; name.maxLength = 63;
  const count = el("input"); count.type = "number"; count.min = "1"; count.max = "10"; count.value = "1";
  const key = el("textarea"); key.placeholder = "ssh-ed25519 AAAA... you@laptop"; key.rows = 3;
  const userData = el("textarea"); userData.placeholder = "#cloud-config"; userData.rows = 6;
  try { key.value = localStorage.getItem("pistack.sshkey") || ""; } catch (e) { /* private window */ }
  let data = null;
  let failed = "";

  const steps = [
    { key: "details", title: "Details", lead: "What to call it and how many. With more than one, each gets the name and a number." },
    { key: "image", title: "Image", lead: "The disk the machine starts from." },
    { key: "size", title: "Size", lead: "How much CPU, memory and root disk it gets." },
    { key: "networks", title: "Networks", lead: "Where it is attached. It gets one address on each network you pick. Without a network it has no address." },
    { key: "groups", title: "Security groups", lead: "Which traffic may reach it. With none picked it gets the project's default group." },
    { key: "access", title: "Access", lead: "How you get in, and what it does on first boot." },
    { key: "review", title: "Review", lead: "Check it over, then launch." },
  ];
  let at = 0;

  const nav = el("div", null, "steps");
  nav.append(el("strong", "Launch a machine"));
  const body = el("div", null, "body");
  const heading = el("h4");
  const lead = el("p", null, "lead");
  const pane = el("div", null, "pane");
  const foot = el("div", null, "foot");
  const sum = el("div", null, "sum");
  const prev = el("button", "Back"); prev.type = "button";
  const next = el("button", "Next"); next.type = "button";
  const launch = el("button", "Launch", "primary"); launch.type = "submit";
  foot.append(sum, prev, next, launch);
  body.append(heading, lead, pane, foot);
  form.append(nav, body);

  const one = (set) => [...set][0] || "";
  const howMany = () => Math.min(10, Math.max(1, parseInt(count.value, 10) || 1));
  const missing = () => {
    const out = [];
    if (!name.value.trim()) out.push("details");
    if (!one(state.image)) out.push("image");
    if (!one(state.size)) out.push("size");
    return out;
  };
  const sizeText = (s) => {
    const r = (s && s.spec) || {};
    return (r.vcpus || 0) + " vCPU · " + gib(r.memory_mib || 0) + " GiB · " + (r.disk_gib || 0) + " GiB disk";
  };
