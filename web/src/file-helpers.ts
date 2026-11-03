/**
 * ファイル名・表示まわりの純粋な補助関数。DOMやWebGLに触れない。
 *
 * `main.ts`の表示処理と単体テストで共有する。
 *
 * @packageDocumentation
 */

/** バイト数を短い表記にする。候補一覧とレポートで使う。 */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** FBXの出力パスから拡張子を除いた素朴な名前を取り出す。作品名の初期値に使う。 */
export function fbxStem(path: string): string {
  const file = path.includes("/") ? (path.split("/").pop() ?? path) : path;
  const dot = file.lastIndexOf(".");
  const stem = dot <= 0 ? file : file.slice(0, dot);
  return stem === "" ? "model" : stem;
}

/** 作者欄をカンマ区切りで読み、空要素を除く。 */
export function parseAuthors(raw: string): string[] {
  return raw
    .split(/[,、]/)
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "");
}

/** 作品名から安全な保存名を作る。空なら`avatar.vrm`になる。 */
export function safeFileName(name: string): string {
  // 保存できない文字と制御文字を除く。採用理由はOS間の互換性である。
  const stripped = name.replace(/["*/:<>?\\|%\u0000-\u001f]/g, "").trim().slice(0, 100);
  const base = stripped === "" ? "avatar" : stripped;
  return base.toLowerCase().endsWith(".vrm") ? base : `${base}.vrm`;
}

/**
 * 要求の世代が最新の世代と一致するかを確かめる。古い往復の到着を無視するために使う。
 *
 * @param current - 最新の世代。開く操作と変換要求のどちらでも進む。
 * @param generation - 確かめる要求の世代。
 * @returns 一致する場合に真。
 */
export function isCurrentGeneration(current: number, generation: number): boolean {
  return current === generation;
}
