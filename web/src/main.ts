/**
 * 単一画面UIの配線。ファイル受付・Worker往復・表示更新を担う。
 *
 * 変換の中身はWorker＋wasm、描画は{@link PreviewViewer}に譲り、
 * ここでは状態の持ち回りと表示だけを行う。
 */
import "./style.css";
import { ConversionClient, ConversionError } from "./conversion-client.js";
import type {
  ConversionReport,
  FbxCandidate,
  PhysicsOption,
  PhysicsReport,
  PrefabScanIssue,
} from "./conversion-client.js";
import { fbxStem, formatBytes, isCurrentGeneration, parseAuthors, safeFileName } from "./file-helpers.js";
import {
  decidePrefabSelection,
  isPrefabReady,
  physicsOptionFromSelection,
  prefabBoxState,
  prefabBoxStatusText,
  prefabFetchContinuedText,
  prefabFetchFailedWarnText,
  prefabPromptText,
  selectionFromDecision,
} from "./prefab-selection.js";
import type { PrefabBoxInput, PrefabSelection } from "./prefab-selection.js";
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
  physics: "Prefab物理",
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

/** 物理の選択理由の日本語名。レポートの`selection`と対応する。 */
function physicsSelectionJa(selection: PhysicsReport["selection"]): string {
  switch (selection) {
    case "auto":
      return "自動";
    case "explicit":
      return "明示";
    case "none":
      return "なし";
    case "undetected":
      return "未検出（なし）";
  }
}

/** 視線の種別の日本語名。レポートの`lookAt`と対応する。 */
function physicsLookAtJa(lookAt: PhysicsReport["lookAt"]): string {
  if (lookAt === "bone") {
    return "ボーン";
  }
  if (lookAt === "expression") {
    return "表情";
  }
  return "なし";
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
  private readonly prefabBox = getBlock("prefab-candidates");
  private readonly prefabStatus = getBlock("prefab-status");
  private readonly viewer: PreviewViewer | null;

  private client: ConversionClient | null = null;
  private candidates: readonly FbxCandidate[] = [];
  private selectedFbx: string | null = null;
  /** 対象FBXを参照するPrefab候補（パス昇順）。FBXを変えるたびに取り直す。 */
  private prefabChoices: readonly string[] = [];
  /** 走査中に読めなかったPrefabの記録。黙って隠さず件数と代表を表示する。 */
  private prefabIssues: readonly PrefabScanIssue[] = [];
  /**
   * Prefabの選択状態。パスは使うPrefab、`"none"`は物理なし、`"auto"`は候補0件の
   * 自動継続（レポートは`undetected`になる）、`null`は未選択である。
   *
   * 初期値は{@link decidePrefabSelection}で決める。取得失敗・`unreadable`の存在・
   * 複数件は`null`のまま変換を待たせ、利用者の明示操作でだけ`"none"`か
   * パスに変わる。自動で物理なしに倒さない。
   */
  private prefabSelected: PrefabSelection = null;
  /** Prefab候補の取得中か。取得中も変換を待たせる。 */
  private prefabLoading = false;
  /** Prefab候補の取得失敗文。成功時は`null`である。 */
  private prefabFetchError: string | null = null;
  private vrmBytes: ArrayBuffer | null = null;
  private state: AppState = "idle";
  /** 非同期の往復が古い操作のものかを判別する世代。開く操作と変換要求のどちらでも進める。 */
  private generation = 0;
  /** Prefab候補の取得が古いFBX選択のものかを判別する世代。FBX選択のたびに進める。 */
  private prefabGeneration = 0;

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
    // Prefabが複数件で未選択・取得中のまま変換しない。UI側の選択検証であり、
    // 不正な指定を迂回してもwasm側（`core.rs`）の検証で失敗する。
    this.convertButton.disabled = busy || !convertible || this.selectedFbx === null || !this.isPhysicsReady();
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
    if (detail.stage === "physics") {
      this.appendPhysicsRetry();
    }
    this.errorPanel.hidden = false;
  }

  /**
   * 物理の失敗から物理なしで再変換する導線を足す。
   *
   * 同じFBX・設定で`physics: "none"`を指定し直す。T-poseの適用と視線も
   * 省略されることを文面に含める。選択状態も物理なしに寄せる。
   */
  private appendPhysicsRetry(): void {
    const note = document.createElement("p");
    note.textContent =
      "Prefab由来の物理を使わず、T-poseの適用と視線（lookAt）も省略して再変換できます";
    const retry = document.createElement("button");
    retry.type = "button";
    retry.dataset["testid"] = "retry-without-physics";
    retry.textContent = "Prefab由来の物理を使わず再変換";
    retry.addEventListener("click", () => {
      // 再変換の指定と画面の選択を一致させ、世代の古いPrefab応答で戻らないようにする。
      this.prefabSelected = "none";
      this.renderPrefabBox();
      this.refreshButtons();
      void this.convert("none");
    });
    this.errorPanel.append(note, retry);
  }

  /** Prefabの選択が定まり変換できるかを確かめる。取得中・複数件の未選択は待たせる。 */
  private isPhysicsReady(): boolean {
    return isPrefabReady(this.prefabSelected, this.prefabLoading);
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
    this.prefabGeneration += 1;
    this.client?.terminate();
    this.client = null;
    this.resetPackageView();
    return this.generation;
  }

  /** 新しいファイルのためにパッケージ由来の表示を捨てる。設定欄は残す。 */
  private resetPackageView(): void {
    this.candidates = [];
    this.selectedFbx = null;
    this.prefabChoices = [];
    this.prefabIssues = [];
    this.prefabSelected = null;
    this.prefabLoading = false;
    this.prefabFetchError = null;
    this.vrmBytes = null;
    this.viewer?.clear();
    this.candidatesBox.replaceChildren();
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent = "パッケージを開くと候補が並びます";
    this.candidatesBox.append(hint);
    this.prefabBox.replaceChildren();
    const prefabHint = document.createElement("p");
    prefabHint.className = "hint";
    prefabHint.textContent = "FBXを選ぶと候補が並びます";
    this.prefabBox.append(prefabHint);
    this.prefabStatus.textContent = "FBXを選ぶとPrefab候補を取得します";
    this.prefabStatus.dataset["state"] = "idle";
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
        // 候補が一意なら選ぶ操作を挟まず、Prefab候補の確定後に変換まで進める。
        this.selectFbx(only.path, true);
        this.setState("selecting", "FBXを1件検出しました。Prefab候補を取得しています…");
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
        this.setStatusText("Prefab候補を取得しています…");
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
   * FBXを選び直す。Prefab候補は捨てて取り直す。
   *
   * 古いFBX向けの非同期応答が現在の選択を変えないよう、Prefab世代で判別する。
   * 変換の世代とは別立てにし、既存の変換・プレビューの世代管理に触れない。
   *
   * @param path - 選ぶFBXの出力パス。
   * @param autoConvert - Prefab選択の確定後に変換まで進める。FBX自動選択の初期値に使う。
   */
  private selectFbx(path: string, autoConvert = false): void {
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
    // Prefab選択を破棄し、古い非同期応答が現在の選択を変えないように世代を進める。
    this.prefabGeneration += 1;
    const generation = this.prefabGeneration;
    this.prefabChoices = [];
    this.prefabIssues = [];
    this.prefabSelected = null;
    this.prefabLoading = true;
    this.prefabFetchError = null;
    this.renderPrefabBox();
    this.refreshButtons();
    void this.loadPrefabCandidates(path, generation, autoConvert);
  }

  /** Prefab世代が最新のFBX選択と一致するかを確かめる。古い往復の到着を無視するために使う。 */
  private isCurrentPrefab(generation: number, fbxPath: string): boolean {
    return this.prefabGeneration === generation && this.selectedFbx === fbxPath;
  }

  /**
   * 対象FBXのPrefab候補を取り出し、選択状態を決める。
   *
   * 最初の判断は{@link decidePrefabSelection}に一任する。取得失敗・`unreadable`の
   * 存在・複数件は未選択のまま変換を待たせ、利用者の明示操作を要求する。
   * 自動で物理なしに倒さない。取得失敗時は失敗文を残し、FBX一意の自動変換も行わない。
   *
   * @param fbxPath - 対象FBXの出力パス。
   * @param generation - 要求時のPrefab世代。古い応答の到着を無視するために使う。
   * @param autoConvert - 選択の確定後に変換まで進める。FBX自動選択の初期値に使う。
   */
  private async loadPrefabCandidates(fbxPath: string, generation: number, autoConvert: boolean): Promise<void> {
    const client = this.client;
    if (client === null) {
      return;
    }
    try {
      const report = await client.prefabCandidates(fbxPath, () => {
        if (this.isCurrentPrefab(generation, fbxPath)) {
          this.setStatusText("Prefab候補を取得しています…");
        }
      });
      if (!this.isCurrentPrefab(generation, fbxPath)) {
        return;
      }
      // 候補はパス昇順に並べ直し、既存FBX選択と同じ操作感にする。
      this.prefabChoices = [...report.candidates].sort();
      this.prefabIssues = [...report.unreadable];
      this.prefabFetchError = null;
      this.prefabSelected = selectionFromDecision(
        decidePrefabSelection({
          candidates: this.prefabChoices,
          unreadableCount: this.prefabIssues.length,
          fetchFailed: false,
        }),
      );
      this.prefabLoading = false;
      this.renderPrefabBox();
      this.refreshButtons();
      if (autoConvert && this.isPhysicsReady()) {
        // FBX自動選択の続きであり、Prefab選択が確定している場合だけ変換する。
        void this.convert();
        return;
      }
      this.setStatusText(this.prefabPrompt());
    } catch (error) {
      if (!this.isCurrentPrefab(generation, fbxPath)) {
        return;
      }
      // 中断は`cancel`側で表示を戻し済みのため触れない。
      if (error instanceof ConversionError && error.stage === "cancel") {
        return;
      }
      // 取得失敗時は停止し、利用者の明示操作を待つ。自動で物理なしに倒さず、
      // FBX一意の自動変換も行わない。失敗文は画面に残す。
      const detail = toFailureDetail(error);
      this.prefabChoices = [];
      this.prefabIssues = [];
      this.prefabSelected = null;
      this.prefabLoading = false;
      this.prefabFetchError = detail.message;
      this.renderPrefabBox();
      this.refreshButtons();
      this.setStatusText(this.prefabPrompt());
    }
  }

  /** Prefab選択の確定後に利用者へ示す促し文を返す。文言の決定は純粋関数に従う。 */
  private prefabPrompt(): string {
    return prefabPromptText({
      candidates: this.prefabChoices,
      unreadableCount: this.prefabIssues.length,
      fetchFailed: this.prefabFetchError !== null,
      selection: this.prefabSelected,
    });
  }

  /** 枠内表示の決定に使う入力をまとめる。判断の重複を避けるための入口。 */
  private prefabBoxInput(): PrefabBoxInput {
    return {
      candidates: this.prefabChoices,
      unreadableCount: this.prefabIssues.length,
      fetchFailed: this.prefabFetchError !== null,
      selection: this.prefabSelected,
      loading: this.prefabLoading,
      fetchError: this.prefabFetchError,
    };
  }

  /**
   * Prefab候補の一覧を描く。複数件はFBX選択と同じラジオ操作にする。
   *
   * 0件は`auto`継続の旨、1件は自動選択したPrefab、取得失敗時は停止の旨と
   * 明示の続行操作を示す。`unreadable`がある場合は件数と代表パスを警告として残す。
   * 表示の区分と言葉は純粋関数に従い、ここでは結果の表示だけを行う。
   */
  private renderPrefabBox(): void {
    this.prefabBox.replaceChildren();
    const input = this.prefabBoxInput();
    const state = prefabBoxState(input);
    if (state.kind === "loading") {
      const hint = document.createElement("p");
      hint.className = "hint";
      hint.textContent = "Prefab候補を取得しています…";
      this.prefabBox.append(hint);
      this.prefabStatus.textContent = prefabBoxStatusText(input);
      this.prefabStatus.dataset["state"] = "loading";
      return;
    }
    if (state.kind === "fetch-failed-pending") {
      const warn = document.createElement("p");
      warn.className = "prefab-warn";
      warn.dataset["testid"] = "prefab-fetch-error";
      warn.textContent = prefabFetchFailedWarnText(state.error);
      this.prefabBox.append(warn);
      const cont = document.createElement("button");
      cont.type = "button";
      cont.dataset["testid"] = "prefab-continue-without";
      cont.textContent = "Prefab由来の物理を使わず続行";
      cont.addEventListener("click", () => {
        this.prefabSelected = "none";
        this.renderPrefabBox();
        this.refreshButtons();
        this.setStatusText(this.prefabPrompt());
      });
      this.prefabBox.append(cont);
      this.prefabStatus.textContent = prefabBoxStatusText(input);
      this.prefabStatus.dataset["state"] = "error";
      this.appendPrefabUnreadable();
      return;
    }
    if (state.kind === "fetch-failed-continued") {
      const note = document.createElement("p");
      note.className = "hint";
      note.dataset["testid"] = "prefab-fetch-continued";
      note.textContent = prefabFetchContinuedText(state.error);
      this.prefabBox.append(note);
      this.prefabStatus.textContent = prefabBoxStatusText(input);
      this.prefabStatus.dataset["state"] = "ready";
      this.appendPrefabUnreadable();
      return;
    }
    if (state.kind === "auto-undetected") {
      const note = document.createElement("p");
      note.className = "hint";
      note.textContent = "対応Prefabを検出できないため物理なしで出力します";
      this.prefabBox.append(note);
      this.prefabStatus.textContent = prefabBoxStatusText(input);
      this.prefabStatus.dataset["state"] = "ready";
      this.appendPrefabUnreadable();
      return;
    }
    if (state.kind === "auto-prefab") {
      const use = document.createElement("p");
      use.dataset["testid"] = "prefab-auto";
      use.textContent = `使用Prefab（自動）: ${state.prefab}`;
      this.prefabBox.append(use);
      this.prefabStatus.textContent = prefabBoxStatusText(input);
      this.prefabStatus.dataset["state"] = "ready";
      this.appendPrefabUnreadable();
      return;
    }
    if (state.reason === "unreadable") {
      const note = document.createElement("p");
      note.className = "prefab-warn";
      note.textContent = "読めなかったPrefabがあるため自動で進めません。使うPrefabを選ぶか、物理を使わない場合は「使わない」を選んでください";
      this.prefabBox.append(note);
    }
    for (const path of this.prefabChoices) {
      const label = document.createElement("label");
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "prefab-candidate";
      radio.value = path;
      radio.dataset["testid"] = "prefab-option";
      radio.checked = this.prefabSelected === path;
      radio.addEventListener("change", () => {
        this.prefabSelected = path;
        this.renderPrefabBox();
        this.refreshButtons();
        this.setStatusText(this.prefabPrompt());
      });
      const pathText = document.createElement("span");
      pathText.textContent = path;
      label.append(radio, pathText);
      this.prefabBox.append(label);
    }
    const noneLabel = document.createElement("label");
    const noneRadio = document.createElement("input");
    noneRadio.type = "radio";
    noneRadio.name = "prefab-candidate";
    noneRadio.value = "__none";
    noneRadio.dataset["testid"] = "prefab-none";
    noneRadio.checked = this.prefabSelected === "none";
    noneRadio.addEventListener("change", () => {
      this.prefabSelected = "none";
      this.renderPrefabBox();
      this.refreshButtons();
      this.setStatusText(this.prefabPrompt());
    });
    const noneText = document.createElement("span");
    noneText.textContent = "Prefab由来の物理を使わない";
    noneLabel.append(noneRadio, noneText);
    this.prefabBox.append(noneLabel);
    this.prefabStatus.textContent = prefabBoxStatusText(input);
    this.prefabStatus.dataset["state"] = this.prefabSelected === null ? "undecided" : "ready";
    this.appendPrefabUnreadable();
  }

  /** 読めなかったPrefabの記録を件数と代表パスで警告する。無い場合は何もしない。 */
  private appendPrefabUnreadable(): void {
    if (this.prefabIssues.length === 0) {
      return;
    }
    const first = this.prefabIssues[0];
    const warn = document.createElement("p");
    warn.className = "prefab-warn";
    warn.dataset["testid"] = "prefab-unreadable";
    warn.textContent =
      first === undefined
        ? `読めなかったPrefabが${this.prefabIssues.length}件あります`
        : `読めなかったPrefabが${this.prefabIssues.length}件あります（例: ${first.path}）`;
    this.prefabBox.append(warn);
  }

  /**
   * 画面の選択状態から変換指定の物理を取り出す。判断の写しは共有関数に従う。
   *
   * 候補1件の自動選択と0件の自動継続は`"auto"`のまま渡し、利用者の明示選択だけを
   * `{"prefab": ...}`にする。いずれもwasm側で候補一致を検証する。
   * 未選択（複数件の放置など）で呼ぶと`null`を返し、呼び出し側で変換を待たせる。
   *
   * @returns 変換要求に入れる物理指定。未選択の場合は`null`である。
   */
  private currentPhysicsOption(): PhysicsOption | null {
    const decision = decidePrefabSelection({
      candidates: this.prefabChoices,
      unreadableCount: this.prefabIssues.length,
      fetchFailed: this.prefabFetchError !== null,
    });
    return physicsOptionFromSelection(this.prefabSelected, decision);
  }

  /**
   * 選んだFBXで変換する。設定を変えた後の再変換も同じ入口を使う。
   *
   * 複数Prefabで未選択・取得中のまま変換しない。物理失敗からの再変換は
   * `physicsOverride`に`"none"`を渡し、同じFBX・設定で物理なしにする。
   *
   * @param physicsOverride - 物理指定の上書き。省略時は画面の選択状態を使う。
   */
  private async convert(physicsOverride?: PhysicsOption): Promise<void> {
    const client = this.client;
    const fbxPath = this.selectedFbx;
    if (client === null || fbxPath === null || this.state === "opening" || this.state === "converting") {
      return;
    }
    if (physicsOverride === undefined && !this.isPhysicsReady()) {
      this.setStatusText("使うPrefabを選んでください");
      return;
    }
    const physics = physicsOverride ?? this.currentPhysicsOption();
    if (physics === null) {
      // 未選択のまま変換しない。`isPhysicsReady`を通った場合は到達しないが、
      // 取得中の選択変更に備えて待たせる側に倒す。
      this.setStatusText("使うPrefabを選んでください");
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
          physics,
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

  /** レポート（計数値と物理）を表で示す。 */
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
      ["物理Prefab", report.physics.prefab ?? "なし"],
      ["物理の選択", physicsSelectionJa(report.physics.selection)],
      ["スプリング", String(report.physics.springs)],
      ["ジョイント", String(report.physics.joints)],
      ["コライダー", String(report.physics.colliders)],
      ["コライダーグループ", String(report.physics.groups)],
      ["T-pose適用", String(report.physics.tposeApplied)],
      ["T-poseスキップ", String(report.physics.tposeSkipped)],
      ["視線", physicsLookAtJa(report.physics.lookAt)],
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
