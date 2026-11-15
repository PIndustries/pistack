function quotaInfo(resource) {
  return QUOTAS.find(q => q[0] === resource) || [resource, resource, ""];
}
function quotaValue(resource, n) {
  const scale = quotaInfo(resource)[2];
  if (scale === "mib") return gib(n) + " GiB";
  if (scale === "gib") return n + " GiB";
  if (scale === "bytes") return fmtBytes(n);
  return String(n);
}
