function ingressHosts(item) {
  return ((item.spec && item.spec.rules) || []).map(r => r.host).filter(Boolean).join(", ");
}
