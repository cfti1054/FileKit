/** 경로의 마지막 요소 (\ 와 / 모두 구분자) */
export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}

/** 확장자(점 없음, 소문자). 없으면 빈 문자열 */
export function extOf(path: string): string {
  const name = baseName(path);
  const i = name.lastIndexOf(".");
  return i <= 0 ? "" : name.slice(i + 1).toLowerCase();
}
