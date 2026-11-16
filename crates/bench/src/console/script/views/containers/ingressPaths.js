function ingressPaths(item) {
  const out = [];
  for (const rule of (item.spec && item.spec.rules) || []) {
    for (const path of rule.paths || []) {
      const backend = path.backend || {};
      const dest = backend.service ? backend.service + ":" + backend.port : "";
      out.push(dest ? (path.path || "/") + " · " + dest : (path.path || "/"));
    }
  }
  return out.join(", ");
}
