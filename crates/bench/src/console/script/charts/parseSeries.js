function parseSeries(key) {
  const i = key.indexOf("{");
  const out = { name: i < 0 ? key : key.slice(0, i), labels: {} };
  if (i >= 0) {
    const re = /(\w+)="((?:[^"\\]|\\.)*)"/g;
    let m;
    while ((m = re.exec(key.slice(i)))) out.labels[m[1]] = m[2];
  }
  return out;
}
// The server keeps the last points of every series. One read serves every
// chart on the page for a while.
let sampleCache = { at: 0, rows: [] };
