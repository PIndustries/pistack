function templateImages(item) {
  const template = item.spec && item.spec.template;
  return containerImages(template && template.spec);
}
