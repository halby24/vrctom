/**
 * 変換Worker。wasmの初期化は一度だけ行い、開いたパッケージを保持する。
 *
 * 中断はmain側の`terminate`に任せ、専用の中断手順は持たない。
 * `cancel`要求は処理せず、無視する。
 */
import init, { WebPackage } from "./wasm/vrctom_wasm.js";
import {
  isMainToWorkerMessage,
  parseConversionReport,
  parseFbxCandidates,
} from "./conversion-client.js";
import type {
  MainToWorkerMessage,
  WorkerToMainMessage,
} from "./conversion-client.js";

/** `convert`要求だけを抜き出した型。設定の受け渡しに使う。 */
type ConvertRequest = Extract<MainToWorkerMessage, { readonly type: "convert" }>;

/** wasmの初期化。一度だけ行い、並行した要求で共有する。 */
let initialized: Promise<void> | null = null;
/** 開いているパッケージ。再変換のために保持する。 */
let currentPackage: WebPackage | null = null;

/** wasmを実行できる状態にする。二度目以降は完了済みの約束を返す。 */
function ensureInit(): Promise<void> {
  if (initialized === null) {
    initialized = init().then(() => undefined);
  }
  return initialized;
}

/**
 * 応答をmainへ送る。
 *
 * @param message - 送る応答。
 * @param transfer - 所有権ごと渡すバイト列。VRMの複写を増やさないために使う。
 */
function post(message: WorkerToMainMessage, transfer?: readonly ArrayBuffer[]): void {
  if (transfer === undefined) {
    postMessage(message);
  } else {
    postMessage(message, { transfer: [...transfer] });
  }
}

/**
 * 未知の失敗から段階・文面・対象パスを文字列として取り出す。
 *
 * wasm側は`stage`と`path`を付けた`Error`で失敗するが、形を断定せず取り出す。
 *
 * @param id - 失敗した要求の`id`。
 * @param error - 捕まえた未知の値。
 * @returns mainへ送る`error`応答。
 */
function toErrorMessage(id: number, error: unknown): WorkerToMainMessage {
  let stage = "package";
  let message = "不明なエラーが発生した";
  let path: string | undefined;
  if (error instanceof Error) {
    if (error.message !== "") {
      message = error.message;
    }
    const stageValue: unknown = Reflect.get(error, "stage");
    if (typeof stageValue === "string" && stageValue !== "") {
      stage = stageValue;
    }
    const pathValue: unknown = Reflect.get(error, "path");
    if (typeof pathValue === "string") {
      path = pathValue;
    }
  } else if (typeof error === "string" && error !== "") {
    message = error;
  }
  return path === undefined
    ? { type: "error", id, stage, message }
    : { type: "error", id, stage, message, path };
}

/**
 * パッケージを開き、FBX候補を返す。直前に重い処理の段階を知らせる。
 *
 * 以前に開いたパッケージは破棄して置き換える。
 *
 * @param id - 要求の`id`。
 * @param bytes - `.unitypackage`のバイト列。転送で受け取ったもの。
 */
async function handleOpen(id: number, bytes: ArrayBuffer): Promise<void> {
  post({ type: "progress", id, stage: "package" });
  try {
    await ensureInit();
    currentPackage?.free();
    currentPackage = null;
    const opened = new WebPackage(new Uint8Array(bytes));
    currentPackage = opened;
    const raw: unknown = JSON.parse(opened.fbxCandidates());
    post({ type: "opened", id, candidates: parseFbxCandidates(raw) });
  } catch (error) {
    currentPackage = null;
    post(toErrorMessage(id, error));
  }
}

/**
 * 選んだFBXを変換し、VRMとレポートを返す。直前に重い処理の段階を知らせる。
 *
 * VRMバイト列は転送で渡す。`Converted`は使い終わったら破棄する。
 *
 * @param id - 要求の`id`。
 * @param request - `convert`要求全体。設定の真偽値をwasm側の符号へ写す。
 */
async function handleConvert(id: number, request: ConvertRequest): Promise<void> {
  post({ type: "progress", id, stage: "convert" });
  try {
    await ensureInit();
    if (currentPackage === null) {
      post({ type: "error", id, stage: "package", message: "パッケージが開かれていないため変換できない" });
      return;
    }
    const optionsJson = JSON.stringify({
      name: request.options.name,
      authors: [...request.options.authors],
      expressions: request.options.expressions ? "vrc" : "none",
    });
    const converted = currentPackage.convert(request.fbxPath, optionsJson);
    try {
      // 所有権ごと取り出し、全体を占める裏側の領域は複写せず転送する。
      const source: Uint8Array = converted.takeVrm();
      const backing: ArrayBufferLike = source.buffer;
      const vrm: ArrayBuffer =
        backing instanceof ArrayBuffer && source.byteOffset === 0 && backing.byteLength === source.byteLength
          ? backing
          : // 部分ビューの場合だけ転送できる範囲を切り出す。
            source.slice().buffer;
      const raw: unknown = JSON.parse(converted.report);
      post({ type: "converted", id, vrm, report: parseConversionReport(raw) }, [vrm]);
    } finally {
      converted.free();
    }
  } catch (error) {
    post(toErrorMessage(id, error));
  }
}

/** 受信した要求を振り分ける。`cancel`はmain側の破棄に任せて無視する。 */
function handleMessage(data: unknown): void {
  if (!isMainToWorkerMessage(data)) {
    return;
  }
  switch (data.type) {
    case "open":
      void handleOpen(data.id, data.bytes);
      break;
    case "convert":
      void handleConvert(data.id, data);
      break;
    case "cancel":
      break;
  }
}

// DOM libの型では`addEventListener("message")`が`MessageEvent`を渡す。
// 実行時はWorkerとして動くため、そのまま受信の入口にする。
addEventListener("message", (event: MessageEvent) => {
  // `MessageEvent.data`は型上`any`のため、`unknown`に受けてから検証する。
  const data: unknown = event.data;
  handleMessage(data);
});
