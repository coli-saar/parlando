import { defineConfig } from "vite";

// The locally linked SDK and the game must share one React dispatcher.
export default defineConfig({ resolve: { dedupe: ["react", "react-dom"] } });
