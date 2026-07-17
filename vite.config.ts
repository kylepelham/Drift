import { defineConfig } from "vite"
import solid from "vite-plugin-solid"
import tailwindcss from "@tailwindcss/vite"

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  clearScreen: false,
  optimizeDeps: { entries: ["index.html"] },
  server: { port: 5180, strictPort: true, watch: { ignored: ["**/examples/**"] } },
  build: { target: "es2022" },
})
