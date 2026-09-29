import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/** Builds the deliberately small participant client served by the live test server. */
export default defineConfig({
  base: "./",
  plugins: [react()],
  resolve: {
    dedupe: ["react", "react-dom"]
  },
  build: {
    outDir: "dist",
    emptyOutDir: true
  }
});
