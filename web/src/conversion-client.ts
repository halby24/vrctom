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

/** 変換設定。UIの入力欄と対応する。 */
export interface ConvertOptions {
  /** 作品名。空ならFBXのstemに倒す（wasm側で行う）。 */
  readonly name: string;
  /** 作者列。空要素はwasm側で除き、残らなければ`unknown`になる。 */
  readonly authors: readonly string[];
  /** VRChat向け表情を生成するか。 */
  readonly expressions: boolean;
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
}

/** 変換結果。VRMバイト列とレポートを持つ。 */
export interface ConversionResult {
  /** VRM 1.0のGLBバイト列。呼び出し側が所有する。 */
  readonly vrm: ArrayBuffer;
  /** 変換結果のレポート。 */
  readonly report: ConversionReport;
}

/** main→workerの要求。一意の`id`で応答と対応付ける。 */
export type MainToWorkerMessage =
  | { readonly type: "open"; readonly id: number; readonly bytes: ArrayBuffer }
  | { readonly type: "convert"; readonly id: number; readonly fbxPath: string; readonly options: ConvertOptions }
  | { readonly type: "cancel"; readonly id: number };

/** worker→mainの応答。`id`は要求のものと一致する。 */
export type WorkerToMainMessage =
  | { readonly type: "opened"; readonly id: number; readonly candidates: readonly FbxCandidate[] }
  | { readonly type: "converted"; readonly id: number; readonly vrm: ArrayBuffer; readonly report: ConversionReport }
  | { readonly type: "error"; readonly id: number; readonly stage: string; readonly message: string; readonly path?: string }
  | { readonly type: "progress"; readonly id: number; readonly stage: string };

/**
 * 変換の失敗。wasm側の段階・文面・対象パスを持ち歩く。
 *
 * 段階は`package`・`options`・`fbx`・`model`・`material`・`export`のいずれかであり、
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
 * @param value - `fbx_candidates()`のJSONを解釈した値。
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
 * レポートのJSON値を検証して返す。
 *
 * @param value - `Converted.report`のJSONを解釈した値。
 * @returns 検証済みのレポート。
 * @throws `ConversionError` — 必要な項目が欠けている場合。
 */
export function parseConversionReport(value: unknown): ConversionReport {
  if (!isRecord(value)) {
    throw new ConversionError("export", "レポートの読み取りに失敗した");
  }
  const { fbx, bytes, nodes, meshes, skins, materials, images, expressions, warnings } = value;
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
  return { fbx, bytes, nodes, meshes, skins, materials, images, expressions, warnings: [...warnings] };
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
        typeof options["expressions"] === "boolean"
      );
    }
    case "cancel":
      return true;
    default:
      return false;
  }
}

/**
 * worker→mainの応答値を検証する。`converted`のレポートはここでは検証せず、
 * 解決時に{@link parseConversionReport}で検証する。
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
        if (request.kind !== "convert") {
          return;
        }
        this.pending.delete(data.id);
        try {
          // レポートはパース済みに近い形（検証済みオブジェクト）で渡す。
          request.resolve({ vrm: data.vrm, report: parseConversionReport(data.report) });
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
