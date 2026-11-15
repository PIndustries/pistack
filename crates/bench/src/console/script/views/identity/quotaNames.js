// The resources in a report, in that order, with an unknown name last.
function quotaNames(report) {
  const have = Object.keys((report && report.quotas) || {});
  const known = QUOTAS.map(q => q[0]).filter(r => have.includes(r));
  return known.concat(have.filter(r => !known.includes(r)).sort());
}
// One resource: its bar and `used of limit`, or `used` alone with no limit.
