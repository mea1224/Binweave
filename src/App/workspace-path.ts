// Keep canonical paths in storage/process calls; present familiar Windows paths
// in editable controls, including records created by earlier app versions.
export function displayWorkspacePath(path: string): string {
  if (/^\\\\\?\\UNC\\/i.test(path)) return `\\\\${path.slice(8)}`;
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) return path.slice(4);
  return path;
}