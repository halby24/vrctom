/**
 * Workerとの送受信の型、境界での実行時検証、Workerの寿命管理。
 *
 * 変換の中身はWorker側（`worker.ts`＋wasm）が担い、ここでは送受信の型と
 * Workerの生成・終了だけを受け持つ。外部（Worker・wasm由来のJSON）から来る値は
 * `unknown`として受け、利用前に検証する。
 *
 * @packageDocumentation
 */

/** FBX候補1件。パス昇順で並ぶ。 */
export interface FbxCandidate {
  /** 出力パス（`Assets/...`）。 */
  readonly path: string;
  /** 実体のバイト数。 */
  readonly size: number;
}

/** Prefab走査で読めなかった1件。候補とは別に報告する。 */
export interface PrefabScanIssue {
  /** 出力パス（`Assets/...`）。 */
  readonly path: string;
  /** 読めなかった理由の日本語診断。 */
  readonly reason: string;
}

/** 対象FBXを参照するPrefabの走査結果。候補はパス昇順である。 */
export interface PrefabCandidatesReport {
  /** 対象FBXの出力パス（`Assets/...`）。 */
  readonly fbx: string;
  /** 対象FBXのGUID。 */
  readonly guid: string;
  /** 対象FBXを参照するPrefabの出力パス列。 */
  readonly candidates: readonly string[];
  /** 走査中に解析できなかったPrefabの記録列。 */
  readonly unreadable: readonly PrefabScanIssue[];
}

/** Prefab入口の一覧で使う解決状態。 */
export type PrefabTargetState =
  | "resolved"
  | "noModel"
  | "unsupportedMultipleFbx"
  | "requiresPrefabResolution"
  | "unresolved";

/** Prefab入口の一覧で使う変換対象FBX1件。 */
export interface PrefabTargetFbx {
  /** 資産パス（`Assets/...`）。 */
  readonly path: string;
  /** 資産GUID。 */
  readonly guid: string;
}

/** Prefab1件の解決結果。 */
export interface PrefabTargetCandidate {
  /** 解決したPrefabの資産パス（`Assets/...`）。 */
  readonly prefab: string;
  /** 解決結果の状態。 */
  readonly state: PrefabTargetState;
  /** 変換対象のFBX。`resolved`の場合だけ存在する。 */
  readonly fbx: PrefabTargetFbx | null;
  /** ネストPrefabの資産パス列。空でも必ず出す。 */
  readonly nestedPrefabs: readonly string[];
  /** 非FBXモデルや種別不明の解決済み参照の資産パス列。 */
  readonly otherReferences: readonly string[];
  /** GUID欠落・索引不在・不正GUIDの参照の記録列。 */
  readonly unresolvedReferences: readonly string[];
}

/** Prefab入口の一覧で読めなかった1件。 */
export interface PrefabTargetIssue {
  /** 出力パス（`Assets/...`）。 */
  readonly path: string;
  /** 読めなかった理由の日本語診断。 */
  readonly reason: string;
}

/** 全Prefabの走査結果。候補はパス昇順である。 */
export interface PrefabTargetsReport {
  /** 解決できたPrefabの列。 */
  readonly candidates: readonly PrefabTargetCandidate[];
  /** 読めなかったPrefabの記録列。 */
  readonly unreadable: readonly PrefabTargetIssue[];
}

/** 物理の使い分け。未指定は`auto`と同じである。 */
export type PhysicsOption = "auto" | "none" | { readonly prefab: string };

/** レポート側の物理の選択理由。 */
export type PhysicsSelection = "auto" | "explicit" | "none" | "undetected";

/** Prefabレポート側の物理の選択理由。Prefab APIでは`target`である。 */
export type PrefabPhysicsSelection = "target";

/** レポート側の視線の種別。無い場合は`null`になる。 */
export type PhysicsLookAt = "bone" | "expression";

/** 変換結果に含まれる物理の構造化情報。 */
export interface PhysicsReport {
  /** 使ったPrefabパス。物理なしの場合は`null`である。 */
  readonly prefab: string | null;
  /** 選択理由。 */
  readonly selection: PhysicsSelection;
  /** 出力したspring数。 */
  readonly springs: number;
  /** 出力したjoint総数。 */
  readonly joints: number;
  /** 出力したcollider数。 */
  readonly colliders: number;
  /** 出力したcolliderグループ数。 */
  readonly groups: number;
  /** T-poseで上書きした骨数。 */
  readonly tposeApplied: number;
  /** T-poseで飛ばした骨数。 */
  readonly tposeSkipped: number;
  /** 視線の種別。無い場合は`null`である。 */
  readonly lookAt: PhysicsLookAt | null;
}

/** Prefab変換結果に含まれる物理の構造化情報。 */
export interface PrefabPhysicsReport {
  /** 使ったPrefabパス。物理なしの場合は`null`である。 */
  readonly prefab: string | null;
  /** 選択理由。Prefab APIでは`target`である。 */
  readonly selection: PrefabPhysicsSelection;
  /** 出力したspring数。 */
  readonly springs: number;
  /** 出力したjoint総数。 */
  readonly joints: number;
  /** 出力したcollider数。 */
  readonly colliders: number;
  /** 出力したcolliderグループ数。 */
  readonly groups: number;
  /** T-poseで上書きした骨数。 */
  readonly tposeApplied: number;
  /** T-poseで飛ばした骨数。 */
  readonly tposeSkipped: number;
  /** 視線の種別。無い場合は`null`である。 */
  readonly lookAt: PhysicsLookAt | null;
}

/** 変換設定。UIの入力欄と対応する。 */
export interface ConvertOptions {
  /** 作品名。空ならFBXのstemに倒す（wasm側で行う）。 */
  readonly name: string;
  /** 作者列。空要素はwasm側で除き、残らなければ`unknown`になる。 */
  readonly authors: readonly string[];
  /** VRChat向け表情を生成するか。 */
  readonly expressions: boolean;
  /**
   * 物理の使い分け。省略時は`auto`と同じである。
   *
   * `auto`は候補1件なら自動選択、複数なら失敗、0件なら物理なしになる。
   * 明示のPrefabパスは候補一致が必須であり、`none`は物理なしである。
   */
  readonly physics?: PhysicsOption;
}

/** 変換結果のレポート。wasm側のJSONと対応する。 */
export interface ConversionReport {
  /** 変換したFBXの出力パス。 */
  readonly fbx: string;
  /** VRMバイト列の大きさ。 */
  readonly bytes: number;
  /** ノード数。 */
  readonly nodes: number;
  /** メッシュ数。 */
  readonly meshes: number;
  /** スキン数。 */
  readonly skins: number;
  /** 材質数。 */
  readonly materials: number;
  /** 画像数。 */
  readonly images: number;
  /** 表情数。 */
  readonly expressions: number;
  /** 警告文。失敗とは区別する。 */
  readonly warnings: readonly string[];
  /** 物理の構造化情報。件数と選択理由を持つ。 */
  readonly physics: PhysicsReport;
}

/** Prefab変換設定。UIの入力欄と対応する。 */
export interface ConvertPrefabOptions {
  /** 作品名。空なら解決したFBXのstemに倒す（wasm側で行う）。 */
  readonly name: string;
  /** 作者列。空要素はwasm側で除き、残らなければ`unknown`になる。 */
  readonly authors: readonly string[];
  /** 表情を生成するか。真なら`descriptor`、偽なら`none`になる。 */
  readonly expressions: boolean;
  /** 基準姿勢の焼き込みを行うかどうか。 */
  readonly baseline: boolean;
  /** 物理を導出して適用するかどうか。 */
  readonly physics: boolean;
}

/** Prefab変換結果に含まれる材質割当の構造化情報。 */
export interface PrefabAssignmentReport {
  /** 解決に使った記述方式。無い場合は`null`である。 */
  readonly mode: "instance" | "direct" | null;
  /** 有効レンダラ数。 */
  readonly activeMeshes: number;
  /** 除外したメッシュ数。 */
  readonly excludedMeshes: number;
  /** 除外したメッシュのModel名。 */
  readonly excludedNames: readonly string[];
  /** 材質プールの件数。 */
  readonly materialPool: number;
}

/** Prefab変換結果に含まれる基準姿勢の構造化情報。 */
export interface PrefabBaselineReport {
  /** 焼き込んだメッシュ数。 */
  readonly meshes: number;
  /** 焼き込んだ非ゼロチャンネル数。 */
  readonly shapes: number;
}

/** Prefab変換結果のレポート。wasm側のJSONと対応する。 */
export interface PrefabConversionReport extends Omit<ConversionReport, "physics"> {
  /** 解決したPrefabの資産パス（`Assets/...`）。 */
  readonly prefab: string;
  /** 変換対象のFBXのGUID。 */
  readonly fbxGuid: string;
  /** ネストPrefabの資産パス列。空でも必ず出す。 */
  readonly nestedPrefabs: readonly string[];
  /** 材質割当の報告。 */
  readonly assignment: PrefabAssignmentReport;
  /** 基準姿勢の報告。 */
  readonly baseline: PrefabBaselineReport;
  /** 物理の構造化情報。選択理由は`target`である。 */
  readonly physics: PrefabPhysicsReport;
}

/** 変換結果。VRMバイト列とレポートを持つ。 */
export interface ConversionResult {
  /** VRM 1.0のGLBバイト列。呼び出し側が所有する。 */
  readonly vrm: ArrayBuffer;
  /** 変換結果のレポート。 */
  readonly report: ConversionReport;
}

/** Prefab変換結果。VRMバイト列とPrefabレポートを持つ。 */
export interface PrefabConversionResult {
  /** VRM 1.0のGLBバイト列。呼び出し側が所有する。 */
  readonly vrm: ArrayBuffer;
  /** Prefab変換結果のレポート。 */
  readonly report: PrefabConversionReport;
}

/** main→workerの要求。一意の`id`で応答と対応付ける。 */
export type MainToWorkerMessage =
  | { readonly type: "open"; readonly id: number; readonly bytes: ArrayBuffer }
  | { readonly type: "convert"; readonly id: number; readonly fbxPath: string; readonly options: ConvertOptions }
  | { readonly type: "prefabCandidates"; readonly id: number; readonly fbxPath: string }
  | { readonly type: "prefabTargets"; readonly id: number }
  | { readonly type: "convertPrefab"; readonly id: number; readonly prefabPath: string; readonly options: ConvertPrefabOptions }
  | { readonly type: "cancel"; readonly id: number };

/** worker→mainの応答。`id`は要求のものと一致する。 */
export type WorkerToMainMessage =
  | { readonly type: "opened"; readonly id: number; readonly candidates: readonly FbxCandidate[] }
  | { readonly type: "converted"; readonly id: number; readonly vrm: ArrayBuffer; readonly report: ConversionReport | PrefabConversionReport }
  | { readonly type: "prefabCandidates"; readonly id: number; readonly report: PrefabCandidatesReport }
  | { readonly type: "prefabTargets"; readonly id: number; readonly report: PrefabTargetsReport }
  | { readonly type: "error"; readonly id: number; readonly stage: string; readonly message: string; readonly path?: string }
  | { readonly type: "progress"; readonly id: number; readonly stage: string };

/**
 * 変換の失敗。wasm側の段階・文面・対象パスを持ち歩く。
 *
 * 段階は`package`・`options`・`fbx`・`model`・`material`・`export`・`physics`・
 * `prefab`・`assignment`のいずれかであり、
 * `cancel`は利用者の中断、`worker`はWorker自体の異常を表す。
 */
export class ConversionError extends Error {
  /** 失敗した段階。 */
  readonly stage: string;
  /** 判明している範囲での対象パス。無い場合に欠ける。 */
  readonly path?: string;

  /**
   * @param stage - 失敗した段階。空にしない。
   * @param message - 日本語の診断文。空にしない。
   * @param path - 対象パス。判明している場合に渡す。
   */
  constructor(stage: string, message: string, path?: string) {
    super(message);
    this.name = "ConversionError";
    this.stage = stage;
    if (path !== undefined) {
      this.path = path;
    }
  }
}

/** 未知の値をレコードとして扱えるかを確かめる。 */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** 有限の非負数かを確かめる。JSON由来の計数値の検証に使う。 */
function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

/**
 * FBX候補のJSON値を検証して返す。
 *
 * @param value - `fbxCandidates()`のJSONを解釈した値。
 * @returns 検証済みの候補列。
 * @throws `ConversionError` — 配列や各項目の形が崩れている場合。
 */
export function parseFbxCandidates(value: unknown): FbxCandidate[] {
  if (!Array.isArray(value)) {
    throw new ConversionError("package", "FBX候補の読み取りに失敗した");
  }
  const candidates: FbxCandidate[] = [];
  for (const entry of value) {
    if (!isRecord(entry) || typeof entry["path"] !== "string" || entry["path"] === "" || !isCount(entry["size"])) {
      throw new ConversionError("package", "FBX候補の読み取りに失敗した");
    }
    candidates.push({ path: entry["path"], size: entry["size"] });
  }
  return candidates;
}

/**
 * レポートのJSON値を検証して返す（wasm境界用）。
 *
 * wasm側は`snake_case`（`tpose_applied`・`look_at`）で送る。Workerが受信直後に
 * 検証して`camelCase`へ写すための入口であり、Worker→client境界の検証には
 * {@link parseClientConversionReport}を使う。二つの境界を混同しない。
 *
 * @param value - `Converted.report`のJSONを解釈した値。
 * @returns 検証済みのレポート。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parseConversionReport(value: unknown): ConversionReport {
  return parseConversionReportWith(value, parsePhysicsReport);
}

/**
 * レポートの検証済み値を受け取り直して検証して返す（Worker→client境界用）。
 *
 * Workerは{@link parseConversionReport}で`camelCase`へ写した済みの値を送るため、
 * ここでは`camelCase`（`tposeApplied`・`lookAt`）を受け付ける。wasm由来の
 * `snake_case`は受け付けず、境界の取り違えを検証で検出する。
 * 受け取る値は`unknown`として扱い、利用前に検証する。
 *
 * @param value - Workerから受信したレポートの値。
 * @returns 検証済みのレポート。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parseClientConversionReport(value: unknown): ConversionReport {
  return parseConversionReportWith(value, parseClientPhysicsReport);
}

/** レポート本体の検証を共有し、物理の写しだけを差し替える。 */
function parseConversionReportWith(
  value: unknown,
  parsePhysics: (physics: unknown) => PhysicsReport,
): ConversionReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const { fbx, bytes, nodes, meshes, skins, materials, images, expressions, warnings, physics } = value;
  if (
    typeof fbx !== "string" ||
    !isCount(bytes) ||
    !isCount(nodes) ||
    !isCount(meshes) ||
    !isCount(skins) ||
    !isCount(materials) ||
    !isCount(images) ||
    !isCount(expressions) ||
    !Array.isArray(warnings) ||
    !warnings.every((entry): entry is string => typeof entry === "string")
  ) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  return {
    fbx,
    bytes,
    nodes,
    meshes,
    skins,
    materials,
    images,
    expressions,
    warnings: [...warnings],
    physics: parsePhysics(physics),
  };
}

/**
 * 物理の報告値を検証して返す（wasm境界用）。
 *
 * wasm側は`snake_case`（`tpose_applied`・`look_at`）で送るため、
 * ここで`camelCase`へ写す。欠落や選択理由の崩れは許さない。
 * Worker→client境界の検証には{@link parseClientPhysicsReport}を使う。
 *
 * @param value - レポート内の`physics`の値。
 * @returns 検証済みの物理報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parsePhysicsReport(value: unknown): PhysicsReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const {
    prefab,
    selection,
    springs,
    joints,
    colliders,
    groups,
    tpose_applied: tposeApplied,
    tpose_skipped: tposeSkipped,
    look_at: lookAt,
  } = value;
  return finishPhysicsReport(
    prefab,
    selection,
    springs,
    joints,
    colliders,
    groups,
    tposeApplied,
    tposeSkipped,
    lookAt,
  );
}

/**
 * 物理の検証済み値を受け取り直して検証して返す（Worker→client境界用）。
 *
 * Workerは{@link parsePhysicsReport}で`camelCase`へ写した済みの値を送るため、
 * ここでは`camelCase`（`tposeApplied`・`lookAt`）を受け付ける。wasm由来の
 * `snake_case`は受け付けず、境界の取り違えを検証で検出する。
 *
 * @param value - Workerから受信した`physics`の値。
 * @returns 検証済みの物理報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parseClientPhysicsReport(value: unknown): PhysicsReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const {
    prefab,
    selection,
    springs,
    joints,
    colliders,
    groups,
    tposeApplied,
    tposeSkipped,
    lookAt,
  } = value;
  return finishPhysicsReport(
    prefab,
    selection,
    springs,
    joints,
    colliders,
    groups,
    tposeApplied,
    tposeSkipped,
    lookAt,
  );
}

/** 物理の共通制約を検証し、報告の形にまとめる。キーの写しは呼び出し側が行う。 */
function finishPhysicsReport(
  prefab: unknown,
  selection: unknown,
  springs: unknown,
  joints: unknown,
  colliders: unknown,
  groups: unknown,
  tposeApplied: unknown,
  tposeSkipped: unknown,
  lookAt: unknown,
): PhysicsReport {
  return finishPhysicsReportWith(prefab, selection, springs, joints, colliders, groups, tposeApplied, tposeSkipped, lookAt, false);
}

/** 物理の共通制約を検証する本体。`allowTarget`が真の場合だけ`target`を受け付ける。 */
function finishPhysicsReportWith(
  prefab: unknown,
  selection: unknown,
  springs: unknown,
  joints: unknown,
  colliders: unknown,
  groups: unknown,
  tposeApplied: unknown,
  tposeSkipped: unknown,
  lookAt: unknown,
  allowTarget: boolean,
): PhysicsReport {
  if (
    !(typeof prefab === "string" || prefab === null) ||
    (typeof prefab === "string" && prefab === "") ||
    (selection !== "auto" &&
      selection !== "explicit" &&
      selection !== "none" &&
      selection !== "undetected" &&
      !(allowTarget && selection === "target")) ||
    !isCount(springs) ||
    !isCount(joints) ||
    !isCount(colliders) ||
    !isCount(groups) ||
    !isCount(tposeApplied) ||
    !isCount(tposeSkipped) ||
    !(lookAt === null || lookAt === "bone" || lookAt === "expression")
  ) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  return {
    prefab,
    selection: selection as PhysicsSelection,
    springs: springs as number,
    joints: joints as number,
    colliders: colliders as number,
    groups: groups as number,
    tposeApplied: tposeApplied as number,
    tposeSkipped: tposeSkipped as number,
    lookAt: lookAt as PhysicsLookAt | null,
  };
}

/**
 * Prefab用の物理の検証済み値を受け取り直して検証して返す。
 *
 * 選択理由は`target`だけを受け付ける。共通制約は{@link finishPhysicsReport}の
 * 本体を`target`許容で再利用する。
 *
 * @param value - Workerから受信した`physics`の値。
 * @returns 検証済みのPrefab物理報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
function parsePrefabPhysicsReport(value: unknown): PrefabPhysicsReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const {
    prefab,
    selection,
    springs,
    joints,
    colliders,
    groups,
    tposeApplied,
    tposeSkipped,
    lookAt,
  } = value;
  const finished = finishPhysicsReportWith(
    prefab,
    selection,
    springs,
    joints,
    colliders,
    groups,
    tposeApplied,
    tposeSkipped,
    lookAt,
    true,
  );
  if ((finished.selection as string) !== "target") {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  return {
    prefab: finished.prefab,
    selection: "target",
    springs: finished.springs,
    joints: finished.joints,
    colliders: finished.colliders,
    groups: finished.groups,
    tposeApplied: finished.tposeApplied,
    tposeSkipped: finished.tposeSkipped,
    lookAt: finished.lookAt,
  };
}

/**
 * Prefab候補の報告値を検証して返す。
 *
 * @param value - `prefabCandidates()`のJSONを解釈した値。
 * @returns 検証済みの候補報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parsePrefabCandidatesReport(value: unknown): PrefabCandidatesReport {
  if (!isRecord(value)) {
    throw new ConversionError("physics", "Prefab候補の読み取りに失敗した");
  }
  const { fbx, guid, candidates, unreadable } = value;
  if (typeof fbx !== "string" || fbx === "" || typeof guid !== "string" || guid === "") {
    throw new ConversionError("physics", "Prefab候補の読み取りに失敗した");
  }
  if (
    !Array.isArray(candidates) ||
    !candidates.every((entry): entry is string => typeof entry === "string" && entry !== "")
  ) {
    throw new ConversionError("physics", "Prefab候補の読み取りに失敗した");
  }
  if (!Array.isArray(unreadable)) {
    throw new ConversionError("physics", "Prefab候補の読み取りに失敗した");
  }
  const issues: PrefabScanIssue[] = [];
  for (const entry of unreadable) {
    if (!isRecord(entry) || typeof entry["path"] !== "string" || entry["path"] === "" || typeof entry["reason"] !== "string" || entry["reason"] === "") {
      throw new ConversionError("physics", "Prefab候補の読み取りに失敗した");
    }
    issues.push({ path: entry["path"], reason: entry["reason"] });
  }
  return { fbx, guid, candidates: [...candidates], unreadable: issues };
}

/** 物理指定の値が送受信できる形かを確かめる。省略時は呼び出し側で`auto`に倒す。 */
function isPhysicsOption(value: unknown): value is PhysicsOption {
  if (value === undefined) {
    return true;
  }
  if (value === "auto" || value === "none") {
    return true;
  }
  if (isRecord(value) && typeof value["prefab"] === "string" && value["prefab"] !== "") {
    return Object.keys(value).length === 1;
  }
  return false;
}

/** 文字列の配列かを確かめる。空配列は通す。 */
function isStringArray(value: unknown): value is readonly string[] {
  return Array.isArray(value) && value.every((entry): entry is string => typeof entry === "string");
}

/** 空でない文字列の配列かを確かめる。空配列は通す。 */
function isNonEmptyStringArray(value: unknown): value is readonly string[] {
  return (
    Array.isArray(value) &&
    value.every((entry): entry is string => typeof entry === "string" && entry !== "")
  );
}

/**
 * Prefab入口の一覧の報告値を検証して返す。
 *
 * wasm・Worker由来のJSONを想定し、`camelCase`を受け付ける。未知の状態は拒否する。
 *
 * @param value - `prefabTargets()`のJSONを解釈した値。
 * @returns 検証済みの一覧報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parsePrefabTargetsReport(value: unknown): PrefabTargetsReport {
  if (!isRecord(value)) {
    throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
  }
  const { candidates, unreadable } = value;
  if (!Array.isArray(candidates) || !Array.isArray(unreadable)) {
    throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
  }
  const verified: PrefabTargetCandidate[] = [];
  for (const entry of candidates) {
    if (!isRecord(entry)) {
      throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
    }
    const { prefab, state, fbx, nestedPrefabs, otherReferences, unresolvedReferences } = entry;
    if (typeof prefab !== "string" || prefab === "") {
      throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
    }
    if (
      state !== "resolved" &&
      state !== "noModel" &&
      state !== "unsupportedMultipleFbx" &&
      state !== "requiresPrefabResolution" &&
      state !== "unresolved"
    ) {
      throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
    }
    let verifiedFbx: PrefabTargetFbx | null = null;
    if (fbx !== null) {
      if (
        !isRecord(fbx) ||
        typeof fbx["path"] !== "string" ||
        fbx["path"] === "" ||
        typeof fbx["guid"] !== "string" ||
        fbx["guid"] === ""
      ) {
        throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
      }
      verifiedFbx = { path: fbx["path"], guid: fbx["guid"] };
    }
    if (
      !isStringArray(nestedPrefabs) ||
      !isStringArray(otherReferences) ||
      !isStringArray(unresolvedReferences)
    ) {
      throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
    }
    verified.push({
      prefab,
      state,
      fbx: verifiedFbx,
      nestedPrefabs: [...nestedPrefabs],
      otherReferences: [...otherReferences],
      unresolvedReferences: [...unresolvedReferences],
    });
  }
  const issues: PrefabTargetIssue[] = [];
  for (const entry of unreadable) {
    if (
      !isRecord(entry) ||
      typeof entry["path"] !== "string" ||
      entry["path"] === "" ||
      typeof entry["reason"] !== "string" ||
      entry["reason"] === ""
    ) {
      throw new ConversionError("prefab", "Prefab一覧の読み取りに失敗した");
    }
    issues.push({ path: entry["path"], reason: entry["reason"] });
  }
  return { candidates: verified, unreadable: issues };
}

/**
 * Prefab変換の材質割当の報告値を検証して返す。
 *
 * @param value - レポート内の`assignment`の値。
 * @returns 検証済みの割当報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
function parsePrefabAssignmentReport(value: unknown): PrefabAssignmentReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const { mode, activeMeshes, excludedMeshes, excludedNames, materialPool } = value;
  if (
    !(mode === null || mode === "instance" || mode === "direct") ||
    !isCount(activeMeshes) ||
    !isCount(excludedMeshes) ||
    !isStringArray(excludedNames) ||
    !isCount(materialPool)
  ) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  return {
    mode,
    activeMeshes,
    excludedMeshes,
    excludedNames: [...excludedNames],
    materialPool,
  };
}

/**
 * Prefab変換の基準姿勢の報告値を検証して返す。
 *
 * @param value - レポート内の`baseline`の値。
 * @returns 検証済みの基準姿勢報告。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
function parsePrefabBaselineReport(value: unknown): PrefabBaselineReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const { meshes, shapes } = value;
  if (!isCount(meshes) || !isCount(shapes)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  return { meshes, shapes };
}

/**
 * Prefab変換レポートのJSON値を検証して返す（Worker→client境界用）。
 *
 * wasm側は`camelCase`で送るため、ここでは`camelCase`を受け付ける。
 * 受け取る値は`unknown`として扱い、利用前に検証する。
 *
 * @param value - Workerから受信したレポートの値。
 * @returns 検証済みのPrefabレポート。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parsePrefabConversionReport(value: unknown): PrefabConversionReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const {
    prefab,
    fbx,
    fbxGuid,
    nestedPrefabs,
    bytes,
    nodes,
    meshes,
    skins,
    materials,
    images,
    expressions,
    warnings,
    assignment,
    baseline,
    physics,
  } = value;
  if (
    typeof prefab !== "string" ||
    prefab === "" ||
    typeof fbx !== "string" ||
    fbx === "" ||
    typeof fbxGuid !== "string" ||
    fbxGuid === "" ||
    !isStringArray(nestedPrefabs) ||
    !isCount(bytes) ||
    !isCount(nodes) ||
    !isCount(meshes) ||
    !isCount(skins) ||
    !isCount(materials) ||
    !isCount(images) ||
    !isCount(expressions) ||
    !Array.isArray(warnings) ||
    !warnings.every((entry): entry is string => typeof entry === "string")
  ) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  return {
    prefab,
    fbx,
    fbxGuid,
    nestedPrefabs: [...nestedPrefabs],
    bytes,
    nodes,
    meshes,
    skins,
    materials,
    images,
    expressions,
    warnings: [...warnings],
    assignment: parsePrefabAssignmentReport(assignment),
    baseline: parsePrefabBaselineReport(baseline),
    physics: parsePrefabPhysicsReport(physics),
  };
}

/**
 * main→workerの要求値を検証する。Worker側で受信の正当性を確かめるために使う。
 *
 * @param value - 受信した値。
 * @returns 要求として扱える場合に真。
 */
export function isMainToWorkerMessage(value: unknown): value is MainToWorkerMessage {
  if (!isRecord(value) || typeof value["id"] !== "number" || !Number.isInteger(value["id"])) {
    return false;
  }
  switch (value["type"]) {
    case "open":
      return value["bytes"] instanceof ArrayBuffer;
    case "convert": {
      if (typeof value["fbxPath"] !== "string" || value["fbxPath"] === "" || !isRecord(value["options"])) {
        return false;
      }
      const options = value["options"];
      return (
        typeof options["name"] === "string" &&
        Array.isArray(options["authors"]) &&
        options["authors"].every((entry): entry is string => typeof entry === "string") &&
        typeof options["expressions"] === "boolean" &&
        isPhysicsOption(options["physics"])
      );
    }
    case "prefabCandidates":
      return typeof value["fbxPath"] === "string" && value["fbxPath"] !== "";
    case "prefabTargets":
      return true;
    case "convertPrefab": {
      if (
        typeof value["prefabPath"] !== "string" ||
        value["prefabPath"] === "" ||
        !isRecord(value["options"])
      ) {
        return false;
      }
      const options = value["options"];
      return (
        typeof options["name"] === "string" &&
        Array.isArray(options["authors"]) &&
        options["authors"].every((entry): entry is string => typeof entry === "string") &&
        typeof options["expressions"] === "boolean" &&
        typeof options["baseline"] === "boolean" &&
        typeof options["physics"] === "boolean"
      );
    }
    case "cancel":
      return true;
    default:
      return false;
  }
}

/**
 * worker→mainの応答値を検証する。`converted`・`prefabCandidates`・`prefabTargets`のレポートは
 * ここでは検証せず、解決時に{@link parseClientConversionReport}・
 * {@link parsePrefabConversionReport}・{@link parsePrefabCandidatesReport}・
 * {@link parsePrefabTargetsReport}で検証する。`converted`の検証は
 * Worker→client境界の`camelCase`用であり、wasm境界の
 * {@link parseConversionReport}とは別である。
 *
 * @param value - 受信した値。
 * @returns 応答として扱える場合に真。
 */
function isWorkerToMainMessage(value: unknown): value is WorkerToMainMessage {
  if (!isRecord(value) || typeof value["id"] !== "number" || !Number.isInteger(value["id"])) {
    return false;
  }
  switch (value["type"]) {
    case "opened":
      return Array.isArray(value["candidates"]);
    case "converted":
      return value["vrm"] instanceof ArrayBuffer;
    case "prefabCandidates":
      return isRecord(value["report"]);
    case "prefabTargets":
      return isRecord(value["report"]);
    case "error":
      return (
        typeof value["stage"] === "string" &&
        typeof value["message"] === "string" &&
        (value["path"] === undefined || typeof value["path"] === "string")
      );
    case "progress":
      return typeof value["stage"] === "string";
    default:
      return false;
  }
}

/** 要求の待ち合わせ。種別ごとに解決値が異なる。 */
type PendingRequest =
  | {
      readonly kind: "open";
      readonly resolve: (candidates: readonly FbxCandidate[]) => void;
      readonly reject: (error: ConversionError) => void;
      readonly onProgress?: (stage: string) => void;
    }
  | {
      readonly kind: "convert";
      readonly resolve: (result: ConversionResult) => void;
      readonly reject: (error: ConversionError) => void;
      readonly onProgress?: (stage: string) => void;
    }
  | {
      readonly kind: "prefabCandidates";
      readonly resolve: (report: PrefabCandidatesReport) => void;
      readonly reject: (error: ConversionError) => void;
      readonly onProgress?: (stage: string) => void;
    }
  | {
      readonly kind: "prefabTargets";
      readonly resolve: (report: PrefabTargetsReport) => void;
      readonly reject: (error: ConversionError) => void;
      readonly onProgress?: (stage: string) => void;
    }
  | {
      readonly kind: "convertPrefab";
      readonly resolve: (result: PrefabConversionResult) => void;
      readonly reject: (error: ConversionError) => void;
      readonly onProgress?: (stage: string) => void;
    };

/**
 * 変換Workerの持ち主。要求に一意の`id`を付けて往復し、終了時に破棄する。
 *
 * Workerの生成は呼び出し側が行い、ここでは受け取ったWorkerを使い終わるまで借りる。
 * `terminate`後は再利用できない。
 */
export class ConversionClient {
  private readonly worker: Worker;
  private nextId = 1;
  private readonly pending = new Map<number, PendingRequest>();
  private terminated = false;

  /**
   * @param worker - 変換専用のWorker。所有権はこのclientへ移る。
   */
  constructor(worker: Worker) {
    this.worker = worker;
    this.worker.onmessage = (event: MessageEvent) => {
      // `MessageEvent.data`は型上`any`のため、`unknown`に受けてから検証する。
      const data: unknown = event.data;
      this.handleMessage(data);
    };
    this.worker.onerror = () => {
      this.failAll(new ConversionError("worker", "Workerで予期しないエラーが発生した"));
    };
  }

  /**
   * パッケージを開き、FBX候補を返す。バイト列は転送し、呼び出し側に残さない。
   *
   * @param bytes - `.unitypackage`のバイト列。転送後は呼び出し側で使えない。
   * @param onProgress - 重い処理の直前に段階を受け取る場合に渡す。
   * @returns FBX候補列（パス昇順）。
   * @throws `ConversionError` — 展開の失敗・中断・Worker異常の場合。
   */
  open(bytes: ArrayBuffer, onProgress?: (stage: string) => void): Promise<readonly FbxCandidate[]> {
    this.ensureAlive();
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<readonly FbxCandidate[]>((resolve, reject) => {
      const request: PendingRequest = { kind: "open", resolve, reject };
      this.pending.set(id, onProgress === undefined ? request : { ...request, onProgress });
      try {
        this.worker.postMessage({ type: "open", id, bytes } satisfies MainToWorkerMessage, { transfer: [bytes] });
      } catch {
        // 転送に失敗した要求は残さず、その場で失敗させる。
        this.pending.delete(id);
        reject(new ConversionError("worker", "パッケージをWorkerへ渡せなかった"));
      }
    });
  }

  /**
   * 選んだFBXを変換する。VRMバイト列は転送で受け取る。
   *
   * @param fbxPath - 変換するFBXの出力パス（`Assets/...`）。
   * @param options - 変換設定。
   * @param onProgress - 重い処理の直前に段階を受け取る場合に渡す。
   * @returns VRMバイト列とレポート。バイト列の所有権は呼び出し側へ移る。
   * @throws `ConversionError` — 変換の失敗・中断・Worker異常の場合。
   */
  convert(
    fbxPath: string,
    options: ConvertOptions,
    onProgress?: (stage: string) => void,
  ): Promise<ConversionResult> {
    this.ensureAlive();
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<ConversionResult>((resolve, reject) => {
      const request: PendingRequest = { kind: "convert", resolve, reject };
      this.pending.set(id, onProgress === undefined ? request : { ...request, onProgress });
      this.worker.postMessage({ type: "convert", id, fbxPath, options } satisfies MainToWorkerMessage);
    });
  }

  /**
   * 対象FBXを参照するPrefab候補を取り出す。
   *
   * @param fbxPath - 対象FBXの出力パス（`Assets/...`）。
   * @param onProgress - 重い処理の直前に段階を受け取る場合に渡す。
   * @returns 候補報告（候補はパス昇順）。呼び出し側が所有する。
   * @throws `ConversionError` — 走査の失敗・中断・Worker異常の場合。
   */
  prefabCandidates(fbxPath: string, onProgress?: (stage: string) => void): Promise<PrefabCandidatesReport> {
    this.ensureAlive();
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<PrefabCandidatesReport>((resolve, reject) => {
      const request: PendingRequest = { kind: "prefabCandidates", resolve, reject };
      this.pending.set(id, onProgress === undefined ? request : { ...request, onProgress });
      this.worker.postMessage({ type: "prefabCandidates", id, fbxPath } satisfies MainToWorkerMessage);
    });
  }

  /**
   * 全Prefabの解決状態を取り出す。
   *
   * @param onProgress - 重い処理の直前に段階を受け取る場合に渡す。
   * @returns 一覧報告（候補はパス昇順）。呼び出し側が所有する。
   * @throws `ConversionError` — 走査の失敗・中断・Worker異常の場合。
   */
  prefabTargets(onProgress?: (stage: string) => void): Promise<PrefabTargetsReport> {
    this.ensureAlive();
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<PrefabTargetsReport>((resolve, reject) => {
      const request: PendingRequest = { kind: "prefabTargets", resolve, reject };
      this.pending.set(id, onProgress === undefined ? request : { ...request, onProgress });
      this.worker.postMessage({ type: "prefabTargets", id } satisfies MainToWorkerMessage);
    });
  }

  /**
   * 選んだPrefabを変換する。VRMバイト列は転送で受け取る。
   *
   * @param prefabPath - 変換するPrefabの出力パス（`Assets/...`）。
   * @param options - Prefab変換設定。
   * @param onProgress - 重い処理の直前に段階を受け取る場合に渡す。
   * @returns VRMバイト列とPrefabレポート。バイト列の所有権は呼び出し側へ移る。
   * @throws `ConversionError` — 変換の失敗・中断・Worker異常の場合。
   */
  convertPrefab(
    prefabPath: string,
    options: ConvertPrefabOptions,
    onProgress?: (stage: string) => void,
  ): Promise<PrefabConversionResult> {
    this.ensureAlive();
    const id = this.nextId;
    this.nextId += 1;
    return new Promise<PrefabConversionResult>((resolve, reject) => {
      const request: PendingRequest = { kind: "convertPrefab", resolve, reject };
      this.pending.set(id, onProgress === undefined ? request : { ...request, onProgress });
      this.worker.postMessage({ type: "convertPrefab", id, prefabPath, options } satisfies MainToWorkerMessage);
    });
  }

  /**
   * Workerを破棄し、待ち合わせ中の要求を中断として失敗させる。
   *
   * 変換側に専用の中断手順はなく、破棄が中断を兼ねる。呼び出し側は中断を
   * 段階`cancel`の`ConversionError`として受け取る。
   */
  terminate(): void {
    if (this.terminated) {
      return;
    }
    this.terminated = true;
    this.worker.terminate();
    this.failAll(new ConversionError("cancel", "キャンセルしました"));
  }

  /** 破棄済みなら要求を受け付けない。 */
  private ensureAlive(): void {
    if (this.terminated) {
      throw new ConversionError("cancel", "終了した変換は使えません");
    }
  }

  /** 待ち合わせ中の要求をすべて同じ理由で失敗させる。 */
  private failAll(error: ConversionError): void {
    const requests = [...this.pending.values()];
    this.pending.clear();
    for (const request of requests) {
      request.reject(error);
    }
  }

  /** 応答を`id`で待ち合わせに振り分ける。 */
  private handleMessage(data: unknown): void {
    if (!isWorkerToMainMessage(data)) {
      return;
    }
    const request = this.pending.get(data.id);
    if (request === undefined) {
      return;
    }
    switch (data.type) {
      case "progress":
        request.onProgress?.(data.stage);
        break;
      case "opened":
        if (request.kind !== "open") {
          return;
        }
        this.pending.delete(data.id);
        try {
          request.resolve(parseFbxCandidates(data.candidates));
        } catch (error) {
          request.reject(toConversionError(error));
        }
        break;
      case "converted":
        if (request.kind !== "convert" && request.kind !== "convertPrefab") {
          return;
        }
        this.pending.delete(data.id);
        try {
          // Workerはwasm境界で検証して`camelCase`へ写した済みの値を送るため、
          // ここではWorker→client境界の検証で受け直す。wasm境界の検証を
          // 重ねると`snake_case`要求に当たり成功応答が失敗になる。
          // Prefab変換は`target`選択を含むため専用の検証を使う。
          if (request.kind === "convertPrefab") {
            request.resolve({ vrm: data.vrm, report: parsePrefabConversionReport(data.report) });
          } else {
            request.resolve({ vrm: data.vrm, report: parseClientConversionReport(data.report) });
          }
        } catch (error) {
          request.reject(toConversionError(error));
        }
        break;
      case "prefabCandidates":
        if (request.kind !== "prefabCandidates") {
          return;
        }
        this.pending.delete(data.id);
        try {
          request.resolve(parsePrefabCandidatesReport(data.report));
        } catch (error) {
          request.reject(toConversionError(error));
        }
        break;
      case "prefabTargets":
        if (request.kind !== "prefabTargets") {
          return;
        }
        this.pending.delete(data.id);
        try {
          request.resolve(parsePrefabTargetsReport(data.report));
        } catch (error) {
          request.reject(toConversionError(error));
        }
        break;
      case "error":
        this.pending.delete(data.id);
        request.reject(
          data.path === undefined
            ? new ConversionError(data.stage, data.message)
            : new ConversionError(data.stage, data.message, data.path),
        );
        break;
    }
  }
}

/**
 * 応答処理で投げられた未知の値を`ConversionError`に寄せる。
 *
 * @param error - 検証失敗などの未知の値。
 * @returns 変換失敗として扱えるエラー。
 */
function toConversionError(error: unknown): ConversionError {
  if (error instanceof ConversionError) {
    return error;
  }
  if (error instanceof Error) {
    return new ConversionError("worker", error.message === "" ? "Workerの応答を読めなかった" : error.message);
  }
  return new ConversionError("worker", "Workerの応答を読めなかった");
}
