function diskUse(h) {
  const t = hostCap(h).disk_gib || 0;
  return [t - (hostFree(h).disk_gib || 0), t];
}
