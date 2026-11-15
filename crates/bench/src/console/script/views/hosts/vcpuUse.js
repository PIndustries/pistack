// Used of total. What the scheduler has left room for is the difference.
function vcpuUse(h) {
  const t = hostCap(h).vcpus || 0;
  return [t - (hostFree(h).vcpus || 0), t];
}
