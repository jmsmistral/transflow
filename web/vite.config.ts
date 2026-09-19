import { defineConfig } from "vitest/config";

export default defineConfig({
  base: "./",
  build: { target: "es2022", manifest: true, sourcemap: false },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.tsx"],
    setupFiles: ["src/test-setup.ts"],
    restoreMocks: true,
  },
});
