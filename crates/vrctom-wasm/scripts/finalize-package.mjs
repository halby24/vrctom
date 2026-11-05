// wasm-pack が作った package.json を npm 公開向けに整える。
//
// wasm-pack は Cargo.toml から最小限の項目だけを作るため、
// ESM の exports・公開範囲・リポジトリ表記をここで補う。
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

const directory = process.argv[2];
if (directory === undefined) {
  console.error("使い方: node finalize-package.mjs <pkgディレクトリ>");
  process.exit(1);
}

const path = join(directory, "package.json");
const pkg = JSON.parse(await readFile(path, "utf8"));

pkg.repository = {
  type: "git",
  url: "git+https://github.com/halby24/vrctom.git",
};
pkg.bugs = {
  url: "https://github.com/halby24/vrctom/issues",
};
pkg.files = [
  "vrctom_wasm.js",
  "vrctom_wasm.d.ts",
  "vrctom_wasm_bg.wasm",
  "vrctom_wasm_bg.wasm.d.ts",
  "README.md",
  "LICENSE",
  "THIRD_PARTY_NOTICES.md",
];
pkg.module = "vrctom_wasm.js";
pkg.exports = {
  ".": {
    types: "./vrctom_wasm.d.ts",
    import: "./vrctom_wasm.js",
    default: "./vrctom_wasm.js",
  },
  "./vrctom_wasm_bg.wasm": "./vrctom_wasm_bg.wasm",
  "./package.json": "./package.json",
};
pkg.sideEffects = ["./vrctom_wasm.js"];
pkg.publishConfig = {
  access: "public",
};

await writeFile(path, `${JSON.stringify(pkg, null, 2)}\n`);
