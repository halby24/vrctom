/**
 * 単一画面UIの配線。ファイル受付・Worker往復・表示更新を担う。
 *
 * 変換の中身はWorker＋wasm、描画は{@link PreviewViewer}に譲り、
 * ここでは状態の持ち回りと表示だけを行う。
 */
import "./style.css";
import { ConversionClient, ConversionError } from "./conversion-client.js";
import type { ConversionReport, FbxCandidate } from "./conversion-client.js";
import { fbxStem, formatBytes, isCurrentGeneration, parseAuthors, safeFileName } from "./file-helpers.js";
import { PreviewViewer } from "./viewer.js";

/** 圧縮入力の上限（512MiB）。wasm側の上限と合わせる。 */
const MAX_INPUT_BYTES = 512 * 1024 * 1024;

/** 失敗段階の日本語名。wasm側の段階と対応する。 */
const STAGE_JA: Readonly<Record<string, string>> = {
  package: "パッケージ展開",
  options: "設定",
  fbx: "FBX解析",
  model: "中間モデル",
  material: "材質",
  export: "VRM書き出し",
};

/** 材質欠落を知らせるwasm側の警告文（`core.rs`の固定文）。材質0件時の表示で重複を避けるために除く。 */
const GRAY_WARNING = "材質を読めなかったため灰色で出力する";

/** 画面の状態。`status`要素の`data-state`と一致させる。 */
type AppState = "idle" | "opening" | "selecting" | "converting" | "ready" | "error";

function getButton(testId: string): HTMLButtonElement {
  const element = document.querySelector(`[data-testid="${testId}"]`);
  if (!(element instanceof HTMLButtonElement)) {
    throw new Error(`ボタンが見つからない: ${testId}`);
  }
  return element;
}

function getInput(testId: string): HTMLInputElement {
  const element = document.querySelector(`[data-testid="${testId}"]`);
  if (!(element instanceof HTMLInputElement)) {
    throw new Error(`入力欄が見つからない: ${testId}`);
  }
  return element;
}

function getBlock(testId: string): HTMLElement {
  const element = document.querySelector(`[data-testid="${testId}"]`);
  if (!(element instanceof HTMLElement)) {
    throw new Error(`表示域が見つからない: ${testId}`);
  }
  return element;
}

/** 進行段階の表示文。Workerの`progress`応答と対応する。 */
function progressText(stage: string): string {
  if (stage === "package") {
    return "パッケージを展開しています…";
  }
  if (stage === "convert") {
    return "変換しています…";
  }
  return "処理しています…";
}

/** 失敗の表示材料。段階・文面・対象パスを持つ。 */
interface FailureDetail {
  readonly stage: string | null;
  readonly message: string;
  readonly path?: string;
}

/** 未知の失敗から表示材料を取り出す。 */
function toFailureDetail(error: unknown): FailureDetail {
  if (error instanceof ConversionError) {
    return error.path === undefined
      ? { stage: error.stage, message: error.message }
      : { stage: error.stage, message: error.message, path: error.path };
  }
  if (error instanceof Error) {
    return { stage: null, message: error.message === "" ? "不明なエラーが発生した" : error.message };
  }
  return { stage: null, message: "不明なエラーが発生した" };
}

class App {
  private readonly fileInput = getInput("file-input");
  private readonly dropZone = getBlock("drop-zone");
  private readonly candidatesBox = getBlock("candidates");
  private readonly nameInput = getInput("name-input");
  private readonly authorsInput = getInput("authors-input");
  private readonly expressionsCheck = getInput("expressions-check");
  private readonly convertButton = getButton("convert-button");
  private readonly cancelButton = getButton("cancel-button");
  private readonly downloadButton = getButton("download-button");
  private readonly statusBox = getBlock("status");
  private readonly warningsBox = getBlock("warnings");
  private readonly reportBox = getBlock("report");
  private readonly errorPanel = getBlock("error-panel");
  private readonly viewer: PreviewViewer | null;

  private client: ConversionClient | null = null;
  private candidates: readonly FbxCandidate[] = [];
  private selectedFbx: string | null = null;
  private vrmBytes: ArrayBuffer | null = null;
  private state: AppState = "idle";
  /** 非同期の往復が古い操作のものかを判別する世代。開く操作と変換要求のどちらでも進める。 */
  private generation = 0;

  constructor() {
    const canvas = document.querySelector('[data-testid="preview-canvas"]');
    let viewer: PreviewViewer | null = null;
    if (canvas instanceof HTMLCanvasElement) {
      try {
        viewer = new PreviewViewer(canvas);
      } catch {
        // WebGLが使えない環境でも変換と保存はできるようにする。
        viewer = null;
      }
    }
    this.viewer = viewer;

    this.dropZone.addEventListener("click", (event) => {
      if (event.target !== this.fileInput) {
        this.fileInput.click();
      }
    });
    this.dropZone.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        this.fileInput.click();
      }
    });
    this.dropZone.addEventListener("dragover", (event) => {
      event.preventDefault();
      this.dropZone.classList.add("is-over");
    });
    this.dropZone.addEventListener("dragleave", () => {
      this.dropZone.classList.remove("is-over");
    });
    this.dropZone.addEventListener("drop", (event) => {
      event.preventDefault();
      this.dropZone.classList.remove("is-over");
      const files = event.dataTransfer?.files;
      const first = files?.[0];
      if (first !== undefined) {
        void this.openFile(first);
      }
    });
    this.fileInput.addEventListener("change", () => {
      const first = this.fileInput.files?.[0];
      // 同じファイルを選び直せるように選択を残さない。
      this.fileInput.value = "";
      if (first !== undefined) {
        void this.openFile(first);
      }
    });
    this.convertButton.addEventListener("click", () => {
      void this.convert();
    });
    this.cancelButton.addEventListener("click", () => {
      this.cancel();
    });
    this.downloadButton.addEventListener("click", () => {
      this.download();
    });
    this.refreshButtons();
  }

  /** 状態と表示文をそろえて更新する。 */
  private setState(state: AppState, message: string): void {
    this.state = state;
    this.statusBox.textContent = message;
    this.statusBox.dataset["state"] = state;
    this.refreshButtons();
  }

  /** 変換中などの表示文だけを変える。状態は保つ。 */
  private setStatusText(message: string): void {
    this.statusBox.textContent = message;
  }

  /** ボタンの有効無効と「変換／再変換」の表記を状態に合わせる。 */
  private refreshButtons(): void {
    const busy = this.state === "opening" || this.state === "converting";
    const convertible = this.state === "selecting" || this.state === "ready";
    this.convertButton.disabled = busy || !convertible || this.selectedFbx === null;
    this.convertButton.textContent = this.state === "ready" ? "再変換" : "変換";
    this.cancelButton.disabled = !busy;
    this.downloadButton.disabled = this.state !== "ready" || this.vrmBytes === null;
    this.fileInput.disabled = busy;
    this.nameInput.disabled = busy;
    this.authorsInput.disabled = busy;
    this.expressionsCheck.disabled = busy;
  }

  /** エラー表示を消す。 */
  private clearError(): void {
    this.errorPanel.replaceChildren();
    this.errorPanel.hidden = true;
  }

  /** エラー表示を出す。状態は呼び出し側で決める。 */
  private showError(detail: FailureDetail): void {
    this.errorPanel.replaceChildren();
    const stageJa = detail.stage === null ? null : (STAGE_JA[detail.stage] ?? detail.stage);
    const head = document.createElement("p");
    head.textContent = stageJa === null ? detail.message : `【${stageJa}】${detail.message}`;
    this.errorPanel.append(head);
    if (detail.path !== undefined) {
      const pathLine = document.createElement("p");
      pathLine.textContent = `対象: ${detail.path}`;
      this.errorPanel.append(pathLine);
    }
    this.errorPanel.hidden = false;
  }

  /**
   * パッケージ由来の状態を捨てる。旧パッケージ・旧プレビュー・旧レポート・旧候補を残さない。
   *
   * 無効なファイル・有効な新ファイル・キャンセルで共有し、二重実装を避ける。
   * 表示の初期化は{@link resetPackageView}に譲り、ここでは世代とWorkerの始末だけを足す。
   *
   * @returns 捨てた後に進めた世代。後続の往復が最新のものかを判別するために使う。
   */
  private discardPackage(): number {
    this.generation += 1;
    this.client?.terminate();
    this.client = null;
    this.resetPackageView();
    return this.generation;
  }

  /** 新しいファイルのためにパッケージ由来の表示を捨てる。設定欄は残す。 */
  private resetPackageView(): void {
    this.candidates = [];
    this.selectedFbx = null;
    this.vrmBytes = null;
    this.viewer?.clear();
    this.candidatesBox.replaceChildren();
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent = "パッケージを開くと候補が並びます";
    this.candidatesBox.append(hint);
    this.reportBox.replaceChildren();
    const reportHint = document.createElement("p");
    reportHint.className = "hint";
    reportHint.textContent = "変換すると結果が表示されます";
    this.reportBox.append(reportHint);
    this.warningsBox.replaceChildren();
    const warningsHint = document.createElement("p");
    warningsHint.className = "hint";
    warningsHint.textContent = "警告はありません";
    this.warningsBox.append(warningsHint);
    this.clearError();
  }

  /** 選択したファイルを開き、FBX候補を取り出す。 */
  private async openFile(file: File): Promise<void> {
    if (this.state === "opening" || this.state === "converting") {
      // 処理中の受け付けは世代も表示も変えず、状態文だけで断る。
      this.setStatusText("処理中はファイルを変更できません。完了するかキャンセルしてください");
      return;
    }
    if (!file.name.toLowerCase().endsWith(".unitypackage")) {
      this.discardPackage();
      this.setState("error", "ファイルを開けませんでした");
      this.showError({ stage: null, message: ".unitypackageファイルを選んでください" });
      return;
    }
    if (file.size === 0) {
      this.discardPackage();
      this.setState("error", "ファイルを開けませんでした");
      this.showError({ stage: null, message: "ファイルが空です" });
      return;
    }
    if (file.size > MAX_INPUT_BYTES) {
      this.discardPackage();
      this.setState("error", "ファイルを開けませんでした");
      this.showError({
        stage: null,
        message: `ファイルが512MiBを超えています（${formatBytes(file.size)}）`,
      });
      return;
    }
    const generation = this.discardPackage();
    const client = new ConversionClient(new Worker(new URL("./worker.ts", import.meta.url), { type: "module" }));
    this.client = client;
    this.setState("opening", progressText("package"));
    let bytes: ArrayBuffer;
    try {
      bytes = await file.arrayBuffer();
    } catch {
      if (!this.isCurrent(generation)) {
        return;
      }
      this.setState("error", "ファイルを開けませんでした");
      this.showError({ stage: null, message: "ファイルを読めませんでした" });
      return;
    }
    if (!this.isCurrent(generation)) {
      client.terminate();
      return;
    }
    try {
      const candidates = await client.open(bytes, (stage) => {
        if (this.isCurrent(generation)) {
          this.setStatusText(progressText(stage));
        }
      });
      if (!this.isCurrent(generation)) {
        return;
      }
      this.onOpened(candidates);
    } catch (error) {
      if (!this.isCurrent(generation)) {
        return;
      }
      // 中断は`cancel`側で表示を戻し済みのため触れない。
      if (error instanceof ConversionError && error.stage === "cancel") {
        return;
      }
      this.client?.terminate();
      this.client = null;
      const detail = toFailureDetail(error);
      this.setState("error", "パッケージを開けませんでした");
      this.showError(detail);
    }
  }

  /** 世代が最新の操作と一致するかを確かめる。古い往復の到着を無視するために使う。 */
  private isCurrent(generation: number): boolean {
    return isCurrentGeneration(this.generation, generation);
  }

  /** 候補の受け取り後の分岐。0件は理由を示し、1件は自動で選ぶ。 */
  private onOpened(candidates: readonly FbxCandidate[]): void {
    this.candidates = candidates;
    if (candidates.length === 0) {
      this.setState("error", "FBXが見つかりませんでした");
      this.showError({
        stage: null,
        message: "パッケージ内にFBXが見つかりませんでした。アバターのモデルを含む.unitypackageを選んでください",
      });
      return;
    }
    this.renderCandidates();
    if (candidates.length === 1) {
      const only = candidates[0];
      if (only !== undefined) {
        this.selectFbx(only.path);
        this.setState("selecting", "FBXを1件検出しました。変換を開始します");
        // 候補が一意なら選ぶ操作を挟まず変換まで進める。
        void this.convert();
        return;
      }
    }
    this.setState("selecting", "変換するFBXを選んでください");
  }

  /** 候補の一覧（ラジオ）を描く。 */
  private renderCandidates(): void {
    this.candidatesBox.replaceChildren();
    for (const candidate of this.candidates) {
      const label = document.createElement("label");
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "fbx-candidate";
      radio.value = candidate.path;
      radio.checked = candidate.path === this.selectedFbx;
      radio.addEventListener("change", () => {
        this.selectFbx(candidate.path);
        this.setStatusText("設定を確認して「変換」を押してください");
      });
      const pathText = document.createElement("span");
      pathText.textContent = candidate.path;
      const sizeText = document.createElement("span");
      sizeText.className = "candidate-size";
      sizeText.textContent = `（${formatBytes(candidate.size)}）`;
      label.append(radio, pathText, sizeText);
      this.candidatesBox.append(label);
    }
  }

  /**
   * FBXを選び直す。
   *
   * @param path - 選ぶFBXの出力パス。
   * @param fillName - 作品名が空の場合にstemで埋める。自動選択の初期値に使う。
   */
  private selectFbx(path: string): void {
    this.selectedFbx = path;
    // 利用者が作品名を入力済みなら上書きしない。空のときだけ候補名で埋める。
    if (this.nameInput.value.trim() === "") {
      this.nameInput.value = fbxStem(path);
    }
    // 選択のたびに一覧を作り直すと、クリックした要素がDOMから外れて
    // フォーカスと操作が途切れる。既存のラジオの印だけを合わせる。
    const radios = this.candidatesBox.querySelectorAll('input[type="radio"]');
    for (const radio of radios) {
      if (radio instanceof HTMLInputElement) {
        radio.checked = radio.value === path;
      }
    }
    this.refreshButtons();
  }

  /** 選んだFBXで変換する。設定を変えた後の再変換も同じ入口を使う。 */
  private async convert(): Promise<void> {
    const client = this.client;
    const fbxPath = this.selectedFbx;
    if (client === null || fbxPath === null || this.state === "opening" || this.state === "converting") {
      return;
    }
    // 変換要求ごとに世代を進め、旧変換の往復・プレビュー完了を無効化する。
    // パッケージは開いたまま使い回すため、Workerの破棄は行わない。
    const generation = this.generation + 1;
    this.generation = generation;
    this.setState("converting", progressText("convert"));
    this.clearError();
    try {
      const result = await client.convert(
        fbxPath,
        {
          name: this.nameInput.value,
          authors: parseAuthors(this.authorsInput.value),
          expressions: this.expressionsCheck.checked,
        },
        (stage) => {
          if (this.isCurrent(generation)) {
            this.setStatusText(progressText(stage));
          }
        },
      );
      if (!this.isCurrent(generation)) {
        return;
      }
      this.vrmBytes = result.vrm;
      this.renderReport(result.report);
      this.renderWarnings(result.report);
      this.setState("ready", "変換が完了しました。プレビューを確認してダウンロードしてください");
      // 読み込み失敗はプレビューだけの失敗とし、ダウンロードは可能なままにする。
      if (this.viewer === null) {
        this.showError({ stage: null, message: "プレビューを表示できませんが、VRMのダウンロードは可能です" });
        return;
      }
      try {
        await this.viewer.loadVrm(result.vrm);
        // 読み込み中に別操作が始まっていたら、古い結果で状態を上書きしない。
        if (!this.isCurrent(generation)) {
          return;
        }
      } catch {
        if (!this.isCurrent(generation)) {
          return;
        }
        this.showError({ stage: null, message: "プレビュー表示に失敗しました。VRMのダウンロードは可能です" });
      }
    } catch (error) {
      if (!this.isCurrent(generation)) {
        return;
      }
      // 中断は`cancel`側で表示を戻し済みのため触れない。
      if (error instanceof ConversionError && error.stage === "cancel") {
        return;
      }
      // 変換の失敗後もパッケージは開いたままなので、選び直して再変換できる。
      const detail = toFailureDetail(error);
      this.setState("selecting", "変換に失敗しました。設定やFBXを変えて再変換できます");
      this.showError(detail);
    }
  }

  /** 変換中はWorkerを破棄し、状態を戻す。 */
  private cancel(): void {
    this.discardPackage();
    this.setState("idle", "キャンセルしました。ファイルを選択し直してください");
  }

  /** 変換結果を保存する。 */
  private download(): void {
    if (this.vrmBytes === null || this.state !== "ready") {
      return;
    }
    const blob = new Blob([this.vrmBytes], { type: "model/gltf-binary" });
    const url = URL.createObjectURL(blob);
    try {
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = safeFileName(this.nameInput.value);
      document.body.append(anchor);
      try {
        anchor.click();
      } finally {
        anchor.remove();
      }
    } finally {
      URL.revokeObjectURL(url);
    }
  }

  /** レポート（計数値）を表で示す。 */
  private renderReport(report: ConversionReport): void {
    this.reportBox.replaceChildren();
    const table = document.createElement("table");
    table.className = "report-table";
    const rows: ReadonlyArray<readonly [string, string]> = [
      ["対象FBX", report.fbx],
      ["バイト数", formatBytes(report.bytes)],
      ["ノード", String(report.nodes)],
      ["メッシュ", String(report.meshes)],
      ["スキン", String(report.skins)],
      ["材質", String(report.materials)],
      ["画像", String(report.images)],
      ["表情", String(report.expressions)],
    ];
    for (const [head, body] of rows) {
      const row = document.createElement("tr");
      const headCell = document.createElement("th");
      headCell.textContent = head;
      const bodyCell = document.createElement("td");
      bodyCell.textContent = body;
      row.append(headCell, bodyCell);
      table.append(row);
    }
    this.reportBox.append(table);
  }

  /** 警告の一覧を示す。材質0件の灰色表示は目立たせる。 */
  private renderWarnings(report: ConversionReport): void {
    this.warningsBox.replaceChildren();
    if (report.materials === 0) {
      const note = document.createElement("p");
      note.className = "empty-materials-note";
      note.textContent = "材質を読めなかったため灰色で表示しています";
      this.warningsBox.append(note);
    }
    const rest = report.warnings.filter((entry) => entry !== GRAY_WARNING);
    if (rest.length === 0) {
      if (report.materials !== 0) {
        const hint = document.createElement("p");
        hint.className = "hint";
        hint.textContent = "警告はありません";
        this.warningsBox.append(hint);
      }
      return;
    }
    const list = document.createElement("ul");
    for (const entry of rest) {
      const item = document.createElement("li");
      item.textContent = entry;
      list.append(item);
    }
    this.warningsBox.append(list);
  }
}

new App();
