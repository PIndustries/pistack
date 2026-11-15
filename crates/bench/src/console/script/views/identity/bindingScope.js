function bindingScope(binding) {
  const scope = (binding.spec && binding.spec.scope) || {};
  if (scope.project) return projectName(scope.project);
  const tail = String(binding.name || "").split(":").pop();
  if (tail && tail !== "system") return projectName(tail);
  return "whole cloud";
}
function bindingHolder(binding) {
  const parts = String(binding.name || "").split(":");
  return parts.length > 1 ? parts[0] : shortId(binding.spec && binding.spec.principal);
}
// A binding is shown by name, and deleted by id.
