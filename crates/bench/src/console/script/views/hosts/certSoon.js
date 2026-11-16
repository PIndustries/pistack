function certSoon(h) {
  const t = Date.parse((h && h.status && h.status.cert_expires) || "");
  return !!t && t - Date.now() < 14 * DAY;
}

// The host list, dressed: meters, the disabled pill, a certificate in two
// weeks' time, and rows that open the host page.
