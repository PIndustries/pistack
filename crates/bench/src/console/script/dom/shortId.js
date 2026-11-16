function shortId(id) {
  const m = /^([a-z]+)-[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{4}([0-9a-f]{8})$/.exec(String(id || ""));
  return m ? m[1] + "-" + m[2] : String(id || "");
}
// Long ids inside a message, cut to the short form pi prints.
function shortIds(text) {
  return String(text || "").replace(/\b([a-z]+)-[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{4}([0-9a-f]{8})\b/g, "$1-$2");
}
