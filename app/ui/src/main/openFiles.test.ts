import { describe, expect, it } from "vitest";
import { EVENTS } from "../shared/ipc";
import { createMockIpc } from "../shared/mock";

describe("main:open_files", () => {
  it("이벤트 이름이 계약과 같다", () => {
    expect(EVENTS.mainOpenFiles).toBe("main:open_files");
  });

  it("mock에서 구독하고 받은 파일로 get_formats를 부를 수 있다", async () => {
    const ipc = createMockIpc();
    const got: string[][] = [];
    await ipc.on(EVENTS.mainOpenFiles, (p) => got.push(p.files));
    ipc.emit(EVENTS.mainOpenFiles, { files: ["C:/a/b.png"] });
    expect(got).toEqual([["C:/a/b.png"]]);
    const report = await ipc.getFormats(got[0] ?? [], got[0]?.[0] ?? null);
    expect(report.files).toHaveLength(1);
    expect(report.options.length).toBeGreaterThan(0);
  });
});
