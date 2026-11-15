function machineHost(m) {
  return String((m.status && m.status.host) || "");
}
// Placed machines per host id.
