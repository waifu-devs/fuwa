import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin, searchForWorkspaceRoot } from "vite";

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

// The app ships inside the server, so its version is the server's (for anonymous reports).
const version = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(new URL("../server/Cargo.toml", import.meta.url), "utf8"))?.[1] ?? "dev";

export default defineConfig({
  define: { __FUWA_VERSION__: JSON.stringify(version) },
  plugins: [woff2Only(), react(), tailwindcss()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) },
  },
  server: {
    // The translations (locales/ at the repo root) are shared with the desktop app.
    // (allow replaces Vite's default, so the workspace root goes back in, and nothing wider.)
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), fileURLToPath(new URL("../locales", import.meta.url))] },
    proxy: {
      "^/fuwa\\.v1\\.": { target: fuwa, changeOrigin: true },
      "^/media/": { target: fuwa, changeOrigin: true },
      "^/sso/": { target: fuwa, changeOrigin: true },
      "^/updates/": { target: fuwa, changeOrigin: true },
    },
  },
  build: {
    target: "es2022",
    // Fonts stay files: inlined into the stylesheet, every subset's bytes would block the first paint
    // even though the browser only fetches the subsets a page uses.
    assetsInlineLimit: (file) => (/\.woff2?$/.test(file) ? false : undefined),
    chunkSizeWarningLimit: 1500,
    // Libraries in their own file: the app's code changes far more often, and each stays under the limit.
    // vgpu (WebGPU effects behind the chat) is left out of it: it loads only when someone turns an effect on.
    rolldownOptions: {
      output: {
        codeSplitting: { groups: [{ name: "vendor", test: /node_modules[\\/](?!\.pnpm[\\/](?:vgpu|@vgpu|wgpu-matrix))(?!(?:vgpu|@vgpu|wgpu-matrix)[\\/])/ }] },
      },
    },
  },
});
