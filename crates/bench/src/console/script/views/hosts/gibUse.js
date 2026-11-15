function gibUse(h) {
  const t = hostCap(h).memory_mib || 0;
  return [gib(t - (hostFree(h).memory_mib || 0)), gib(t)];
}
