import { defineConfig } from "vite";
import { viteSingleFile } from "vite-plugin-singlefile";

// One self-contained HTML file: the Rust CLI embeds it and swaps in the data.
export default defineConfig({
  plugins: [viteSingleFile()],
  build: { target: "es2022", assetsInlineLimit: 100_000_000 },
});
