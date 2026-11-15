function containerImages(spec) {
  return ((spec && spec.containers) || []).map(c => c.image).filter(Boolean).join(", ");
}
