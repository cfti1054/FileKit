/** 움직임 사용 여부: Rust 설정과 브라우저 reduced-motion 중 하나라도 끔이면 끈다. */
export function resolveMotion(animationsEnabled: boolean | null | undefined, prefersReduced: boolean): boolean {
  return (animationsEnabled ?? true) && !prefersReduced;
}
