function hostCap(h) { return (h && h.spec && h.spec.capacity) || {}; }
function hostFree(h) { return (h && h.status && h.status.free) || {}; }
