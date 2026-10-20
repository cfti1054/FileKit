import { describe, expect, it } from "vitest";
import {
  blend,
  contrastRatio,
  parseHex,
  relativeLuminance,
  resolveAccent,
  FALLBACK_ACCENT_DARK,
  FALLBACK_ACCENT_LIGHT,
} from "./contrast";

describe("대비 계산", () => {
  it("흑백 휘도와 대비는 0, 1, 21", () => {
    expect(relativeLuminance([0, 0, 0])).toBe(0);
    expect(relativeLuminance([255, 255, 255])).toBeCloseTo(1, 10);
    expect(contrastRatio([0, 0, 0], [255, 255, 255])).toBeCloseTo(21, 5);
    expect(contrastRatio([255, 255, 255], [0, 0, 0])).toBeCloseTo(21, 5);
  });

  it("같은 색의 대비는 1", () => {
    expect(contrastRatio([10, 20, 30], [10, 20, 30])).toBeCloseTo(1, 10);
  });

  it("알려진 값: #767676 대 흰색은 약 4.54", () => {
    const c = contrastRatio([0x76, 0x76, 0x76], [255, 255, 255]);
    expect(c).toBeGreaterThan(4.5);
    expect(c).toBeLessThan(4.6);
  });

  it("parseHex", () => {
    expect(parseHex("#0067C0")).toEqual([0, 0x67, 0xc0]);
    expect(parseHex("#fff")).toEqual([255, 255, 255]);
    expect(parseHex("nope")).toBeNull();
    expect(parseHex("#12345")).toBeNull();
  });

  it("blend", () => {
    expect(blend([0, 0, 0], [255, 255, 255], 0.5)).toEqual([128, 128, 128]);
    expect(blend([10, 20, 30], [200, 200, 200], 0)).toEqual([200, 200, 200]);
  });
});

describe("강조색 대체", () => {
  it("값이 없거나 잘못되면 기본 파란색", () => {
    expect(resolveAccent(null, false)).toBe(FALLBACK_ACCENT_LIGHT);
    expect(resolveAccent(null, true)).toBe(FALLBACK_ACCENT_DARK);
    expect(resolveAccent("garbage", false)).toBe(FALLBACK_ACCENT_LIGHT);
  });

  it("밝은 모드에서 충분히 진한 강조색은 그대로 쓴다", () => {
    expect(resolveAccent("#005A9E", false)).toBe("#005A9E");
  });

  it("밝은 모드에서 노란색은 대비 부족으로 대체", () => {
    expect(resolveAccent("#FFB900", false)).toBe(FALLBACK_ACCENT_LIGHT);
  });

  it("어두운 모드에서 진한 남색은 대비 부족으로 대체, 밝은 하늘색은 사용", () => {
    expect(resolveAccent("#003A70", true)).toBe(FALLBACK_ACCENT_DARK);
    expect(resolveAccent("#60CDFF", true)).toBe("#60CDFF");
  });

  it("기본 파란색 자체가 기준을 통과한다", () => {
    expect(resolveAccent(FALLBACK_ACCENT_LIGHT, false)).toBe(FALLBACK_ACCENT_LIGHT);
    expect(resolveAccent(FALLBACK_ACCENT_DARK, true)).toBe(FALLBACK_ACCENT_DARK);
  });
});
