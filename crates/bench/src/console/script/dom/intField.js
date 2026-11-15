function intField(value, label, min, max) {
  const n = Number(String(value).trim());
  if (!Number.isInteger(n) || n < min || (max != null && n > max)) {
    throw new Error(max == null
      ? label + " must be an integer >= " + min
      : label + " must be an integer from " + min + " to " + max);
  }
  return n;
}
