// WCAG 2.x 상대 휘도·대비 계산과 강조색 대체. 모두 순수 함수.

export type Rgb = readonly [number, number, number];

export const FALLBACK_ACCENT_LIGHT = "#0067C0";
export const FALLBACK_ACCENT_DARK = "#4CC2FF";
export const MIN_CONTRAST = 4.5;

/** 판 배경·글자색의 불투명 근사값(실제 판은 반투명). 대비 계산 기준. */
export const PANEL = {
  light: { background: "#F3F3F3", text: "#1A1A1A" },
  dark: { background: "#2B2B2B", text: "#F5F5F5" },
} as const;

/** "#RRGGBB" 또는 "#RGB" → [r,g,b] (0..255). 형식이 틀리면 null. */
export function parseHex(hex: string): Rgb | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim());
  if (!m || m[1] === undefined) return null;
  let h = m[1];
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

function channel(v: number): number {
  const s = v / 255;
  return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** WCAG 상대 휘도 (0..1) */
export function relativeLuminance(rgb: Rgb): number {
  return 0.2126 * channel(rgb[0]) + 0.7152 * channel(rgb[1]) + 0.0722 * channel(rgb[2]);
}

/** WCAG 대비 비율 (1..21) */
export function contrastRatio(a: Rgb, b: Rgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

/** fg를 alpha(0..1)로 bg 위에 올린 색 */
export function blend(fg: Rgb, bg: Rgb, alpha: number): Rgb {
  return [
    Math.round(fg[0] * alpha + bg[0] * (1 - alpha)),
    Math.round(fg[1] * alpha + bg[1] * (1 - alpha)),
    Math.round(fg[2] * alpha + bg[2] * (1 - alpha)),
  ];
}

/**
 * Windows 강조색을 쓸 수 있는지 판정해 쓸 색을 돌려준다.
 * 조건: (1) 강조색 대 판 배경 4.5:1 이상, (2) 강조색 12% 채움 위 글자색 4.5:1 이상.
 * 못 미치거나 값이 없거나 잘못되면 기본 파란색(밝게 #0067C0, 어둡게 #4CC2FF).
 */
export function resolveAccent(accent: string | null, dark: boolean): string {
  const fallback = dark ? FALLBACK_ACCENT_DARK : FALLBACK_ACCENT_LIGHT;
  if (accent === null) return fallback;
  const rgb = parseHex(accent);
  const panel = dark ? PANEL.dark : PANEL.light;
  const bg = parseHex(panel.background);
  const text = parseHex(panel.text);
  if (rgb === null || bg === null || text === null) return fallback;
  if (contrastRatio(rgb, bg) < MIN_CONTRAST) return fallback;
  if (contrastRatio(text, blend(rgb, bg, 0.12)) < MIN_CONTRAST) return fallback;
  return "#" + rgb.map((v) => v.toString(16).padStart(2, "0")).join("").toUpperCase();
}
