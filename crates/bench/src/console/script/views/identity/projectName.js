function projectName(id) {
  if (!id) return "";
  return projectNames.get(String(id)) || shortId(id);
}
function nameOf(item) {
  return (item && item.name) || (item && item.meta && item.meta.name) || "";
}
// The pages an id on a projects or users line points into: project names,
// and for users the roles and projects the add-role form offers. A page
// that will not answer leaves the short id on the line.
