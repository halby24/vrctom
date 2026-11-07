/**
 * Prefab選択の判断ロジック。DOMやWebGLに触れない純粋関数。
 *
 * `main.ts`の状態遷移と単体テストで共有する。1件自動・複数は選択必須・
 * 0件はauto継続・unreadableは停止して選択要求・取得失敗は停止の判断を
 * 一か所に集め、UI側に重複した分岐を残さない。
 *
 * @packageDocumentation
 */

import type { PhysicsOption } from "./conversion-client.js";

/**
 * Prefabの選択状態。パスは使うPrefab、`"none"`は物理なし、
 * `"auto"`は候補0件の自動継続（レポートは`undetected`になる）、
 * `null`は未選択である。
 */
export type PrefabSelection = string | "none" | "auto" | null;

/** Prefab候補の走査結果の要約。判断に必要な入力だけを持つ。 */
export interface PrefabSelectionInput {
  /** 対象FBXを参照するPrefabの出力パス列（パス昇順）。 */
  readonly candidates: readonly string[];
  /** 走査中に解析できなかったPrefabの件数。 */
  readonly unreadableCount: number;
  /** 候補の取得に失敗したか。 */
  readonly fetchFailed: boolean;
}

/** 選択が定まらない理由。利用者への促し文に使う。 */
export type PrefabPendingReason = "multiple" | "unreadable" | "fetch-failed";

/** Prefab選択の判断結果。 */
export type PrefabDecision =
  | { readonly kind: "auto-prefab"; readonly prefab: string }
  | { readonly kind: "auto-undetected" }
  | { readonly kind: "pending"; readonly reason: PrefabPendingReason };

/**
 * Prefab候補の走査結果から最初の判断を決める。
 *
 * 取得失敗と`unreadable`の存在は候補件数によらず停止し、利用者の選択を待つ。
 * 1件のみで他に異常が無い場合は自動選択、0件で異常が無い場合は`auto`のまま
 * 続ける（wasm側で`undetected`として物理なしになる）。複数件は選択を待つ。
 *
 * @param input - 候補列・解析不能件数・取得失敗の組。
 * @returns 判断結果。選択要求の場合は理由を持つ。
 */
export function decidePrefabSelection(input: PrefabSelectionInput): PrefabDecision {
  if (input.fetchFailed) {
    return { kind: "pending", reason: "fetch-failed" };
  }
  if (input.unreadableCount > 0) {
    return { kind: "pending", reason: "unreadable" };
  }
  if (input.candidates.length === 1) {
    const only = input.candidates[0];
    if (only !== undefined) {
      return { kind: "auto-prefab", prefab: only };
    }
  }
  if (input.candidates.length === 0) {
    return { kind: "auto-undetected" };
  }
  return { kind: "pending", reason: "multiple" };
}

/**
 * 判断結果を画面の選択状態の初期値に写す。
 *
 * 選択要求の場合は`null`（未選択）になり、変換を待たせる。
 *
 * @param decision - {@link decidePrefabSelection}の結果。
 * @returns 選択状態の初期値。
 */
export function selectionFromDecision(decision: PrefabDecision): string | "auto" | null {
  switch (decision.kind) {
    case "auto-prefab":
      return decision.prefab;
    case "auto-undetected":
      return "auto";
    case "pending":
      return null;
  }
}

/**
 * 画面の選択状態から変換指定の物理を取り出す。
 *
 * 1件の自動選択は`"auto"`のまま渡し、利用者が選び直した場合だけ
 * `{"prefab": ...}`にする。未選択は`null`を返し、呼び出し側で変換を待たせる。
 * いずれもwasm側（`core.rs`）で候補一致を検証する。
 *
 * @param selection - 画面の選択状態。
 * @param decision - 現在の入力に対する判断結果。自動か明示かの区別に使う。
 * @returns 変換要求に入れる物理指定。未選択の場合は`null`である。
 */
export function physicsOptionFromSelection(
  selection: PrefabSelection,
  decision: PrefabDecision,
): PhysicsOption | null {
  if (selection === null) {
    return null;
  }
  if (selection === "none" || selection === "auto") {
    return selection;
  }
  if (decision.kind === "auto-prefab" && decision.prefab === selection) {
    return "auto";
  }
  return { prefab: selection };
}

/**
 * 画面の選択状態から利用者への促し文を決める。
 *
 * `main.ts`の状態表示と枠内状態表示で共有する。選択済み（明示パス・`"none"`・
 * `"auto"`）は選択をそのまま文面にし、未選択のときだけ走査結果の判断で
 * 促す。取得失敗後の`"none"`も物理なしの案内になり、「停止しました」を残さない。
 *
 * @param input - 候補列・解析不能件数・取得失敗と画面の選択状態の組。
 * @returns 表示する促し文。
 */
export function prefabPromptText(input: PrefabPromptInput): string {
  if (input.selection === "none") {
    return "Prefab由来の物理を使わず出力します。設定を確認して「変換」を押してください";
  }
  if (input.selection === "auto") {
    return "対応Prefabを検出できないため物理なしで出力します。設定を確認して「変換」を押してください";
  }
  if (input.selection !== null) {
    return `使用Prefab: ${input.selection}。設定を確認して「変換」を押してください`;
  }
  if (input.fetchFailed) {
    return "Prefab候補の取得に失敗しました。「Prefab由来の物理を使わず続行」を選ぶと、物理・T-poseの適用・視線（lookAt）を省略して出力します";
  }
  if (input.unreadableCount > 0) {
    return `読めなかったPrefabがあるため自動で進めません。使うPrefabを選んでください（候補${input.candidates.length}件）。物理を使わない場合は「使わない」を選んでください`;
  }
  return `使うPrefabを選んでください（候補${input.candidates.length}件）。物理を使わない場合は「使わない」を選んでください`;
}

/** 促し文の決定に使う走査結果と選択状態の組。 */
export interface PrefabPromptInput {
  /** 対象FBXを参照するPrefabの出力パス列（パス昇順）。 */
  readonly candidates: readonly string[];
  /** 走査中に解析できなかったPrefabの件数。 */
  readonly unreadableCount: number;
  /** 候補の取得に失敗したか。 */
  readonly fetchFailed: boolean;
  /** 画面の選択状態。 */
  readonly selection: PrefabSelection;
}

/** 枠内表示の区分。選択状態を反映し、取得失敗後の継続を停止と区別する。 */
export type PrefabBoxState =
  | { readonly kind: "loading" }
  | { readonly kind: "fetch-failed-pending"; readonly error: string | null }
  | { readonly kind: "fetch-failed-continued"; readonly error: string | null }
  | { readonly kind: "auto-undetected" }
  | { readonly kind: "auto-prefab"; readonly prefab: string }
  | { readonly kind: "select"; readonly reason: "multiple" | "unreadable" };

/** 枠内表示の決定に使う入力。促し文の入力に取得状態を足したもの。 */
export interface PrefabBoxInput extends PrefabPromptInput {
  /** 候補の取得中か。 */
  readonly loading: boolean;
  /** 候補の取得失敗文。成功時は`null`である。 */
  readonly fetchError: string | null;
}

/**
 * 枠内表示の区分を決める。取得中を最優先し、取得失敗は選択状態で
 * 継続（`"none"`）と停止（未選択）を区別する。自動表示は選択と判断が
 * 一致するときだけ使い、それ以外は選択肢の一覧にする。
 *
 * @param input - 候補列・解析不能件数・取得失敗・選択状態・取得中・失敗文の組。
 * @returns 枠内表示の区分。継続・停止のどちらも失敗文を持つ。
 */
export function prefabBoxState(input: PrefabBoxInput): PrefabBoxState {
  if (input.loading) {
    return { kind: "loading" };
  }
  if (input.fetchFailed) {
    if (input.selection === "none") {
      return { kind: "fetch-failed-continued", error: input.fetchError };
    }
    return { kind: "fetch-failed-pending", error: input.fetchError };
  }
  const decision = decidePrefabSelection({
    candidates: input.candidates,
    unreadableCount: input.unreadableCount,
    fetchFailed: false,
  });
  if (decision.kind === "auto-undetected") {
    return { kind: "auto-undetected" };
  }
  if (decision.kind === "auto-prefab" && input.selection === decision.prefab) {
    return { kind: "auto-prefab", prefab: decision.prefab };
  }
  if (decision.kind === "pending" && decision.reason === "unreadable") {
    return { kind: "select", reason: "unreadable" };
  }
  return { kind: "select", reason: "multiple" };
}

/**
 * 取得失敗の停止警告文を作る。枠内の警告表示で使う。
 *
 * @param error - 取得失敗文。`null`のときは理由を省く。
 * @returns 停止の旨と続行操作の説明を含む警告文。
 */
export function prefabFetchFailedWarnText(error: string | null): string {
  if (error === null) {
    return "Prefab候補の取得に失敗したため停止しました。続けるには「Prefab由来の物理を使わず続行」を選び、物理・T-poseの適用・視線（lookAt）を省略します";
  }
  return `Prefab候補の取得に失敗したため停止しました（${error}）。続けるには「Prefab由来の物理を使わず続行」を選び、物理・T-poseの適用・視線（lookAt）を省略します`;
}

/**
 * 取得失敗後に物理なしを選んだ場合の案内文を作る。「停止しました」を残さず、
 * 失敗理由は括弧内に残す。
 *
 * @param error - 取得失敗文。`null`のときは理由を省く。
 * @returns 失敗理由と物理なしで続行する旨を含む案内文。
 */
export function prefabFetchContinuedText(error: string | null): string {
  if (error === null) {
    return "Prefab候補の取得に失敗しました。Prefab由来の物理を使わず続行します。物理・T-poseの適用・視線（lookAt）を省略して出力します";
  }
  return `Prefab候補の取得に失敗しました（${error}）。Prefab由来の物理を使わず続行します。物理・T-poseの適用・視線（lookAt）を省略して出力します`;
}

/**
 * 枠の状態行（`prefab-status`）の表示文を決める。停止文言は取得失敗で
 * 未選択のときだけ使い、物理なしを選んだ後は促し文に切り替える。
 *
 * @param input - 枠内表示の決定に使う入力と同じ組。
 * @returns 状態行に表示する文面。
 */
export function prefabBoxStatusText(input: PrefabBoxInput): string {
  const state = prefabBoxState(input);
  switch (state.kind) {
    case "loading":
      return "Prefab候補を取得しています…";
    case "fetch-failed-pending":
      return "Prefab候補の取得に失敗したため停止しました";
    case "fetch-failed-continued":
      return prefabPromptText(input);
    case "auto-undetected":
      return "対応Prefabを検出できないため物理なしで出力します";
    case "auto-prefab":
      return `使用Prefab（自動）: ${state.prefab}`;
    case "select":
      return prefabPromptText(input);
  }
}
/**
 * Prefabの選択が定まり変換できるかを確かめる。取得中・未選択のまま変換しない。
 *
 * @param selection - 画面の選択状態。
 * @param loading - 候補の取得中か。
 * @returns 変換できる場合に真。
 */
export function isPrefabReady(selection: PrefabSelection, loading: boolean): boolean {
  return !loading && selection !== null;
}
