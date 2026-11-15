function fmtBytes(n) {
  n = Number(n) || 0;
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return (i === 0 || n >= 100 ? Math.round(n) : Math.round(n * 10) / 10) + " " + units[i];
}
