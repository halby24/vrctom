/**
 * Prefab入口の一覧表示に使う純粋な判断ロジック。DOMやWebGLに触れない。
 *
 * `main.ts`の状態遷移と単体テストで共有する。選択可否・状態文・要約・
 * 初期選択の判断を一か所に集め、UI側に重複した分岐を残さない。
 *
 * @packageDocumentation
 */

import type { PrefabTargetCandidate, PrefabTargetFbx } from "./conversion-client.js";

/** 選択可能なPrefab候補。解決済みで対象FBXを持つ。 */
export type SelectablePrefabTarget = PrefabTargetCandidate & {
  readonly state: "resolved";
  readonly fbx: PrefabTargetFbx;
};

/**
 * 変換対象として選べるかを確かめる。
 *
 * 解決済みで対象FBXを持つ場合だけ真になる。状態だけが`resolved`でも
 * FBXが無い場合は選べない。
 *
 * @param candidate - Prefab1件の解決結果。
 * @returns 選べる場合に真。
 */
export function isSelectable(candidate: PrefabTargetCandidate): candidate is SelectablePrefabTarget {
  return candidate.state === "resolved" && candidate.fbx !== null;
}

/**
 * 解決状態の日本語名を返す。
 *
 * 一覧の各行に添える短い札であり、対象FBXなどの詳細は{@link targetDetails}に譲る。
 *
 * @param candidate - Prefab1件の解決結果。
 * @returns 状態の日本語名。
 */
export function stateLabel(candidate: PrefabTargetCandidate): string {
  switch (candidate.state) {
    case "resolved":
      return candidate.fbx === null ? "対象FBXなし" : "変換可能";
    case "requiresPrefabResolution":
      return "ネストPrefabの解決が必要（未対応）";
    case "unsupportedMultipleFbx":
      return "直接参照するFBXが複数あるため未対応";
    case "unresolved":
      return "参照先を解決できない";
    case "noModel":
      return "FBX参照がない";
  }
}

/** 資産パスをファイル名と親ディレクトリに分けた組。 */
export interface SplitPath {
  /** 末尾のファイル名。 */
  readonly name: string;
  /** 親ディレクトリ（末尾の`/`を含む）。直下の場合は空文字である。 */
  readonly dir: string;
}

/**
 * 資産パスをファイル名と親ディレクトリに分ける。
 *
 * @param path - 資産パス（`Assets/...`）。
 * @returns ファイル名と親ディレクトリの組。
 */
export function splitAssetPath(path: string): SplitPath {
  const slash = path.lastIndexOf("/");
  if (slash < 0) {
    return { name: path, dir: "" };
  }
  return { name: path.slice(slash + 1), dir: path.slice(0, slash + 1) };
}

/**
 * パス列に共通する親ディレクトリを求める。一覧で同じ接頭辞を繰り返さないために使う。
 *
 * ディレクトリ単位で比べ、ファイル名の途中では切らない。
 *
 * @param paths - 資産パス列。
 * @returns 共通の親ディレクトリ（末尾の`/`を含む）。無い場合は空文字である。
 */
export function commonDirectory(paths: readonly string[]): string {
  let common: string[] | null = null;
  for (const path of paths) {
    const dirs = path.split("/").slice(0, -1);
    if (common === null) {
      common = dirs;
      continue;
    }
    let length = 0;
    while (length < common.length && length < dirs.length && common[length] === dirs[length]) {
      length += 1;
    }
    common = common.slice(0, length);
  }
  return common === null || common.length === 0 ? "" : `${common.join("/")}/`;
}

/**
 * 共通の親ディレクトリを取り除いた相対表示を作る。
 *
 * @param path - 資産パス。
 * @param base - {@link commonDirectory}の結果。
 * @returns `base`以下の相対パス。`base`で始まらない場合はそのまま返す。
 */
export function relativeTo(path: string, base: string): string {
  return base !== "" && path.startsWith(base) ? path.slice(base.length) : path;
}

/** 一覧の1行に添える詳細の1項目。 */
export interface TargetDetail {
  /** 項目名。 */
  readonly label: string;
  /** 表示する値。共通ディレクトリを除いた相対パスや件数である。 */
  readonly value: string;
  /** 省略前の完全な値。ツールチップに使う。 */
  readonly title: string;
}

/**
 * 対象FBX・ネスト・未解決参照を項目列にまとめる。
 *
 * パスは共通ディレクトリを除いて短くし、完全なパスは`title`に残す。
 * 長い参照列は件数に畳む。
 *
 * @param candidate - Prefab1件の解決結果。
 * @param base - 一覧全体の共通ディレクトリ。
 * @returns 表示する項目列。情報が無い場合は空である。
 */
export function targetDetails(candidate: PrefabTargetCandidate, base: string): TargetDetail[] {
  const details: TargetDetail[] = [];
  if (candidate.fbx !== null) {
    details.push({ label: "FBX", value: relativeTo(candidate.fbx.path, base), title: candidate.fbx.path });
  }
  const nested = candidate.nestedPrefabs;
  if (nested.length === 1 && nested[0] !== undefined) {
    details.push({ label: "ネストPrefab", value: relativeTo(nested[0], base), title: nested[0] });
  } else if (nested.length > 1) {
    details.push({ label: "ネストPrefab", value: `${nested.length}件`, title: nested.join("\n") });
  }
  const unresolved = candidate.unresolvedReferences;
  if (unresolved.length > 0) {
    details.push({ label: "未解決参照", value: `${unresolved.length}件`, title: unresolved.join("\n") });
  }
  return details;
}

/**
 * 解決できた件数を数える。自動選択の判断に使う。
 *
 * @param candidates - Prefab候補列。
 * @returns 選べる候補の件数。
 */
export function resolvedCount(candidates: readonly PrefabTargetCandidate[]): number {
  return candidates.filter((candidate) => isSelectable(candidate)).length;
}

/**
 * 解決できた候補が1件だけの場合に自動で選ぶパスを決める。
 *
 * 0件・複数件は利用者の明示操作を待つため`null`になる。変換まで自動で進めない。
 *
 * @param candidates - Prefab候補列。
 * @returns 自動選択するPrefabパス。自動選択しない場合は`null`である。
 */
export function initialSelection(candidates: readonly PrefabTargetCandidate[]): string | null {
  const resolved = candidates.filter((candidate) => isSelectable(candidate));
  if (resolved.length !== 1) {
    return null;
  }
  const only = resolved[0];
  return only === undefined ? null : only.prefab;
}

/**
 * 一覧の確定後に利用者へ示す促し文を返す。
 *
 * @param candidates - Prefab候補列。
 * @param unreadableCount - 走査中に読めなかったPrefabの件数。
 * @returns 表示する促し文。
 */
export function targetsPrompt(candidates: readonly PrefabTargetCandidate[], unreadableCount: number): string {
  const resolved = resolvedCount(candidates);
  if (resolved === 0) {
    return "変換可能なPrefabが見つかりませんでした";
  }
  if (resolved === 1) {
    const selected = initialSelection(candidates);
    return selected === null
      ? "Prefabを選ぶと、そのPrefabが参照するFBXを変換します"
      : `変換可能なPrefabを1件検出しました: ${selected}。Prefabを選ぶと、そのPrefabが参照するFBXを変換します`;
  }
  const base = `変換するPrefabを選んでください（変換可能${resolved}件）`;
  return unreadableCount > 0
    ? `${base}（読めなかったPrefabが${unreadableCount}件あります）`
    : base;
}
