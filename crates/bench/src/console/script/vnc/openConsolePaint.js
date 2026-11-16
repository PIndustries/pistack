  function paint(r, pixels) {
    const img = ctx.createImageData(r.w, r.h);
    const view = new DataView(pixels.buffer, pixels.byteOffset, pixels.byteLength);
    for (let i = 0; i < r.w * r.h; i++) {
      const v = pf.big ? view.getUint32(i * 4, false) : view.getUint32(i * 4, true);
      let red = (v >>> pf.redShift) & pf.redMax;
      let green = (v >>> pf.greenShift) & pf.greenMax;
      let blue = (v >>> pf.blueShift) & pf.blueMax;
      if (pf.redMax && pf.redMax !== 255) red = Math.round(red * 255 / pf.redMax);
      if (pf.greenMax && pf.greenMax !== 255) green = Math.round(green * 255 / pf.greenMax);
      if (pf.blueMax && pf.blueMax !== 255) blue = Math.round(blue * 255 / pf.blueMax);
      const o = i * 4;
      img.data[o] = red;
      img.data[o + 1] = green;
      img.data[o + 2] = blue;
      img.data[o + 3] = 255;
    }
    ctx.putImageData(img, r.x, r.y);
  }
