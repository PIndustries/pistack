function fmtNum(n) {
  n = Number(n) || 0;
  return n >= 100 ? String(Math.round(n)) : n >= 10 ? String(Math.round(n * 10) / 10) : String(Math.round(n * 100) / 100);
}

// A series key is `name{label="value",...}`.
