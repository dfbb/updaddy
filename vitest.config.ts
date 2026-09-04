import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  // Vitest 当前携带的 Vite 类型与应用 Vite 版本可能存在重复实例，运行时插件仍兼容。
  plugins: [react() as any],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
  },
});
