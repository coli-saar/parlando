import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  root: "web",
  base: "./",
  plugins: [react()],
  resolve: {
    // Local Parlando installs are symlinked, so force hooks and rendering through
    // the application's single React instance instead of the library workspace's.
    dedupe: ["react", "react-dom"]
  },
  build: {
    outDir: "../dist",
    emptyOutDir: true
  },
  test: {
    root: ".",
    environment: "jsdom",
    globals: true,
    setupFiles: ["./web/src/setupTests.ts"]
  }
});
