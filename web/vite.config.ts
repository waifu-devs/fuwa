import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

// In development the page is served by Vite, so gRPC-Web calls, picture
// uploads and identity providers' answers (/sso/...) to this origin are
// passed to a local fuwa server (FUWA_DEV_URL, default localhost:8080).
const fuwa = process.env.FUWA_DEV_URL ?? "http://localhost:8080";

// The app ships its own copy of the site's font (M PLUS Rounded 1c) so it works
// offline on self-hosted instances. Every browser the app supports reads WOFF2,
// so the older WOFF copies are left out of the build.
function woff2Only(): Plugin {
  return {
    name: "fuwa:woff2-only",
    enforce: "pre",
    transform(code, id) {
      if (!/@fontsource\/.+\.css$/.test(id)) return;
      return code.replace(/,\s*url\([^)]+\.woff\) format\('woff'\)/g, "");
    },
  };
}

export default defineConfig({
  plugins: [woff2Only(), react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: {
    proxy: {
      "^/fuwa\\.v1\\.": { target: fuwa, changeOrigin: true },
      "^/media/": { target: fuwa, changeOrigin: true },
      "^/sso/": { target: fuwa, changeOrigin: true },
    },
  },
  build: {
    target: "es2022",
    chunkSizeWarningLimit: 1500,
    // Libraries in their own file: the app's code changes far more often, and each stays under the limit.
    rolldownOptions: {
      output: {
        codeSplitting: { groups: [{ name: "vendor", test: /node_modules/ }] },
      },
    },
  },
});
