function bareAddr(value) {
  const s = String(value == null ? "" : value).trim();
  const i = s.indexOf("/");
  return i === -1 ? s : s.slice(0, i).trim();
}
