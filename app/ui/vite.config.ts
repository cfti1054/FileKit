import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// 멀티 페이지: index.html = main 창, radial.html = 원형 메뉴 오버레이 창
export default defineConfig({
  plugins: [react()],
  base: "./",
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    rollupOptions: {
      input: { main: "index.html", radial: "radial.html" },
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
