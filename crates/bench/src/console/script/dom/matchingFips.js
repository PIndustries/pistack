function matchingFips(machine, fips) {
  const addrs = {};
  for (const a of (machine.status && machine.status.addresses) || []) {
    addrs[String(a)] = true;
    const bare = bareAddr(a);
    if (bare) addrs[bare] = true;
  }
  return (fips || []).filter(f => {
    const fixed = f.status && f.status.fixed_ip;
    if (!fixed) return false;
    return addrs[String(fixed)] || addrs[bareAddr(fixed)];
  });
}
