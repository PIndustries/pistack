  function pump() {
    while (stage !== "dead") {
      if (stage === "version") {
        const v = take(12);
        if (!v) return;
        let s = "";
        for (const b of v) s += String.fromCharCode(b);
        if (s !== "RFB 003.008\n") { fail("unexpected RFB version"); return; }
        send(new TextEncoder().encode("RFB 003.008\n"));
        stage = "sec-count";
        continue;
      }
      if (stage === "sec-count") {
        const n = take(1);
        if (!n) return;
        if (n[0] === 0) { fail("VNC refused the connection"); return; }
        pending = n[0];
        stage = "sec-types";
        continue;
      }
      if (stage === "sec-types") {
        const types = take(pending);
        if (!types) return;
        let none = false;
        for (const b of types) if (b === 1) none = true;
        if (!none) { fail("VNC security is not None"); return; }
        send(new Uint8Array([1]));
        stage = "sec-result";
        continue;
      }
      if (stage === "sec-result") {
        const r = take(4);
        if (!r) return;
        if (u32(r, 0) !== 0) { fail("VNC security rejected"); return; }
        send(new Uint8Array([1]));
        stage = "init";
        continue;
      }
      if (stage === "init") {
        const h = take(24);
        if (!h) return;
        fbW = u16(h, 0);
        fbH = u16(h, 2);
        if (h[4] !== 32) { fail("framebuffer is " + h[4] + " bpp"); return; }
        pf = {
          big: h[6] === 1,
          redMax: u16(h, 8),
          greenMax: u16(h, 10),
          blueMax: u16(h, 12),
          redShift: h[14],
          greenShift: h[15],
          blueShift: h[16],
        };
        pending = u32(h, 20);
        stage = "name";
        continue;
      }
      if (stage === "name") {
        const name = take(pending);
        if (!name) return;
        canvas.width = fbW;
        canvas.height = fbH;
        status.textContent = fbW + "×" + fbH;
        const enc = new Uint8Array(8);
        enc[0] = 2;
        enc[3] = 1;
        send(enc);
        requestFb(false);
        stage = "msg";
        continue;
      }
      if (stage === "msg") {
        const t = take(1);
        if (!t) return;
        if (t[0] === 0) { stage = "fbu"; continue; }
        if (t[0] === 2) continue;
        if (t[0] === 3) { stage = "cut"; continue; }
        fail("unknown server message " + t[0]);
        return;
      }
      if (stage === "fbu") {
        const h = take(3);
        if (!h) return;
        rectsLeft = u16(h, 1);
        if (!rectsLeft) { requestFb(true); stage = "msg"; continue; }
        stage = "rect";
        continue;
      }
      if (stage === "rect") {
        const h = take(12);
        if (!h) return;
        rect = { x: u16(h, 0), y: u16(h, 2), w: u16(h, 4), h: u16(h, 6), enc: i32(h, 8) };
        if (rect.enc !== 0) { fail("encoding " + rect.enc + " is not Raw"); return; }
        rawLeft = rect.w * rect.h * 4;
        stage = "raw";
        continue;
      }
      if (stage === "raw") {
        const pixels = take(rawLeft);
        if (!pixels) return;
        paint(rect, pixels);
        rectsLeft -= 1;
        if (rectsLeft > 0) stage = "rect";
        else { requestFb(true); stage = "msg"; }
        continue;
      }
      if (stage === "cut") {
        const h = take(7);
        if (!h) return;
        pending = u32(h, 3);
        stage = "cut-body";
        continue;
      }
      if (stage === "cut-body") {
        if (!take(pending)) return;
        stage = "msg";
        continue;
      }
      return;
    }
  }
