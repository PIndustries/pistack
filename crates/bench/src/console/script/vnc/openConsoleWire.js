
  function append(chunk) {
    if (off > 65536) {
      buf = buf.slice(off);
      off = 0;
    }
    const next = new Uint8Array(buf.length - off + chunk.length);
    next.set(buf.subarray(off), 0);
    next.set(chunk, buf.length - off);
    buf = next;
    off = 0;
  }
  function take(n) {
    if (buf.length - off < n) return null;
    const out = buf.subarray(off, off + n);
    off += n;
    return out;
  }
  function u16(a, o) { return (a[o] << 8) | a[o + 1]; }
  function u32(a, o) { return ((a[o] << 24) | (a[o + 1] << 16) | (a[o + 2] << 8) | a[o + 3]) >>> 0; }
  function i32(a, o) { return (a[o] << 24) | (a[o + 1] << 16) | (a[o + 2] << 8) | a[o + 3]; }
  function send(bytes) {
    if (ws && ws.readyState === 1) ws.send(bytes);
  }
  function fail(text) {
    status.textContent = text;
    stage = "dead";
    try { if (ws) ws.close(); } catch (e) {}
  }
  function requestFb(incremental) {
    const m = new Uint8Array(10);
    m[0] = 3;
    m[1] = incremental ? 1 : 0;
    m[6] = (fbW >> 8) & 255;
    m[7] = fbW & 255;
    m[8] = (fbH >> 8) & 255;
    m[9] = fbH & 255;
    send(m);
  }
