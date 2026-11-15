function num(obj, key) {
  if (!obj || obj[key] == null || obj[key] === "") return "";
  return obj[key];
}
