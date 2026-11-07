/**
 * Prefab入口の一覧表示に使う純粋な判断ロジック。DOMやWebGLに触れない。
 *
 * `main.ts`の状態遷移と単体テストで共有する。選択可否・状態文・要約・
 * 初期選択の判断を一か所に集め、UI側に重複した分岐を残さない。
 *
 * @packageDocumentation
 */

import type { PrefabTargetCandidate } from "./conversion-client.js";

/**
 * 変換対象として選べるかを確かめる。
 *
 * 解決済みで対象FBXを持つ場合だけ真になる。状態だけが`resolved`でも
 * FBXが無い場合は選べない。
 *
 * @param candidate - Prefab1件の解決結果。
 * @returns 選べる場合に真。
 */
export function isSelectable(candidate: PrefabTargetCandidate): boolean {
  return candidate.state === "resolved" && candidate.fbx !== null;
}

/**
 * 解決状態の日本語名を返す。
 *
 * 一覧の選択肢に添える短い説明であり、詳細は{@link targetSummary}に譲る。
 *
 * @param candidate - Prefab1件の解決結果。
 * @returns 状態の日本語名。
 */
export function stateLabel(candidate: PrefabTargetCandidate): string {
  switch (candidate.state) {
    case "resolved":
      return candidate.fbx === null
        ? "変換可能（対象FBXが無いため選べない）"
        : `変換可能（対象FBX: ${candidate.fbx.path}）`;
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

/**
 * 対象FBX・ネスト・未解決参照の要約を作る。
 *
 * 一覧の補足行として使い、長い参照列は件数に畳む。
 *
 * @param candidate - Prefab1件の解決結果。
 * @returns 要約文。情報が無い場合は空文字である。
 */
export function targetSummary(candidate: PrefabTargetCandidate): string {
  const parts: string[] = [];
  if (candidate.fbx !== null) {
    parts.push(`対象FBX: ${candidate.fbx.path}`);
  }
  if (candidate.nestedPrefabs.length > 0) {
    parts.push(`ネストPrefab（ジオメトリは出力しない）: ${candidate.nestedPrefabs.join(", ")}`);
  }
  if (candidate.unresolvedReferences.length > 0) {
    const first = candidate.unresolvedReferences[0];
    parts.push(
      first === undefined
        ? `未解決参照${candidate.unresolvedReferences.length}件`
        : `未解決参照${candidate.unresolvedReferences.length}件（例: ${first}）`,
    );
  }
  return parts.join(" / ");
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
  const base = `変換するPrefabを選んでください（変換可能${resolved}件）。Prefabを選ぶと、そのPrefabが参照するFBXを変換します`;
  return unreadableCount > 0
    ? `${base}（読めなかったPrefabが${unreadableCount}件あります）`
    : base;
}
