import { sveltekit } from "@sveltejs/kit/vite";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import topLevelAwait from "vite-plugin-top-level-await";
import wasm from "vite-plugin-wasm";

export default defineConfig({
    plugins: [wasm(), topLevelAwait(), tailwindcss(), sveltekit()],
    server: {
        proxy: {
            "/graphql": {
                target: "http://localhost:8080",
                changeOrigin: true,
                ws: true,
            },
        },
    },
});
