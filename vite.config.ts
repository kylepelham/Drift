import tailwindcss from "@tailwindcss/vite";
import packageJson from "./package.json";
import solid from "vite-plugin-solid";
import { defineConfig } from "vite";

export default defineConfig({
    plugins: [solid(), tailwindcss()],
    clearScreen: false,
    define: {
        __DRIFT_VERSION__: JSON.stringify(packageJson.version),
    },
    // linguist-languages exports names with spaces, which need the es2022 module namespace syntax.
    optimizeDeps: { entries: ["index.html", "app.html"], esbuildOptions: { target: "es2022" } },
    server: { port: 5180, strictPort: true, watch: { ignored: ["**/examples/**"] } },
    // `app.html` is the Drift app's computers page, served on the phone by its local proxy.
    build: { target: "es2022", rollupOptions: { input: { main: "index.html", app: "app.html" } } },
});
