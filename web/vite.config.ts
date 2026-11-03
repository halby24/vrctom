import { defineConfig } from "vite";

export default defineConfig({
  base: "/vrctom/",
  worker: { format: "es" },
  build: { target: "es2022" },
});
