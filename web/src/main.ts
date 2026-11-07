/**
 * 単一画面UIの配線。ファイル受付・Worker往復・表示更新を担う。
 *
 * 変換の中身はWorker＋wasm、描画は{@link PreviewViewer}に譲り、
 * ここでは状態の持ち回りと表示だけを行う。変換入口はPrefabであり、
 * Prefabが参照するFBXを読み取り専用で示す。
 */
import "./style.css";
import { ConversionClient, ConversionError } from "./conversion-client.js";
import type {
  ConversionReport,
  PrefabConversionReport,
  PrefabPhysicsSelection,
  PhysicsReport,
  PhysicsSelection,
  PrefabTargetCandidate,
  PrefabTargetIssue,
  PrefabTargetsReport,
} from "./conversion-client.js";
import { fbxStem, formatBytes, isCurrentGeneration, parseAuthors, safeFileName } from "./file-helpers.js";
import {
  initialSelection,
  isSelectable,
  resolvedCount,
  stateLabel,
  targetSummary,
  targetsPrompt,
} from "./prefab-target.js";
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
  physics: "物理",
  prefab: "Prefab解決",
  assignment: "Prefab割当",
};

/** 材質欠落を知らせるwasm側の警告文（`core.rs`の固定文）。材質0件時の表示で重複を避けるために除く。 */
const GRAY_WARNING = "材質を読めなかったため灰色で出力する";

/** 除外名を一行に並べる上限。超えた分は件数に畳む。 */
const EXCLUDED_NAMES_LIMIT = 5;

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
function physicsSelectionJa(selection: PhysicsSelection | PrefabPhysicsSelection): string {
  switch (selection) {
    case "auto":
      return "自動";
    case "explicit":
      return "明示";
    case "none":
      return "なし";
    case "undetected":
      return "未検出（なし）";
    case "target":
      return "対象Prefab";
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

/** Prefab変換の割当方式の日本語名。レポートの`assignment.mode`と対応する。 */
function assignmentModeJa(mode: PrefabConversionReport["assignment"]["mode"]): string {
  switch (mode) {
    case "instance":
      return "改変内容";
    case "direct":
      return "直接記述";
    case null:
      return "なし";
  }
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
  private readonly baselineCheck = getInput("baseline-check");
  private readonly physicsCheck = getInput("physics-check");
  private readonly resolvedFbx = getBlock("resolved-fbx");
  private readonly prefabNote = getBlock("prefab-note");
  private readonly convertButton = getButton("convert-button");
  private readonly cancelButton = getButton("cancel-button");
  private readonly downloadButton = getButton("download-button");
  private readonly statusBox = getBlock("status");
  private readonly warningsBox = getBlock("warnings");
  private readonly reportBox = getBlock("report");
  private readonly errorPanel = getBlock("error-panel");
  private readonly viewer: PreviewViewer | null;

  private client: ConversionClient | null = null;
  /** Prefab候補の一覧（パス昇順）。パッケージを開くたびに取り直す。 */
  private targets: readonly PrefabTargetCandidate[] = [];
  /** 走査中に読めなかったPrefabの記録。黙って隠さず件数と代表を表示する。 */
  private unreadable: readonly PrefabTargetIssue[] = [];
  /** 選んだPrefabのパス。未選択の場合は`null`である。 */
  private selectedPrefab: string | null = null;
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

  /** 選んだPrefabの解決結果を取り出す。未選択の場合は`null`である。 */
  private selectedTarget(): PrefabTargetCandidate | null {
    if (this.selectedPrefab === null) {
      return null;
    }
    const found = this.targets.find((candidate) => candidate.prefab === this.selectedPrefab);
    return found ?? null;
  }

  /** ボタンの有効無効と「変換／再変換」の表記を状態に合わせる。 */
  private refreshButtons(): void {
    const busy = this.state === "opening" || this.state === "converting";
    const convertible = this.state === "selecting" || this.state === "ready";
    const selected = this.selectedTarget();
    const ready = selected !== null && isSelectable(selected);
    this.convertButton.disabled = busy || !convertible || !ready;
    this.convertButton.textContent = this.state === "ready" ? "再変換" : "変換";
    this.cancelButton.disabled = !busy;
    this.downloadButton.disabled = this.state !== "ready" || this.vrmBytes === null;
    this.fileInput.disabled = busy;
    this.nameInput.disabled = busy;
    this.authorsInput.disabled = busy;
    this.expressionsCheck.disabled = busy;
    this.baselineCheck.disabled = busy;
    this.physicsCheck.disabled = busy;
    for (const radio of this.candidatesBox.querySelectorAll('input[type="radio"]')) {
      if (radio instanceof HTMLInputElement) {
        // 処理中は選べる候補も触らせず、旧結果を新選択の結果と取り違えないようにする。
        radio.disabled = busy || radio.dataset["selectable"] !== "true";
      }
    }
    this.syncTargetRadios();
  }

  /**
   * ラジオの印を選んだPrefabに合わせる。一覧を作り直さず印だけを動かし、
   * フォーカスと操作を途切れさせない。処理中のクリックで印だけが動いた
   * 場合の戻しにも使う。
   */
  private syncTargetRadios(): void {
    const radios = this.candidatesBox.querySelectorAll('input[type="radio"]');
    for (const radio of radios) {
      if (radio instanceof HTMLInputElement) {
        radio.checked = radio.value === this.selectedPrefab;
      }
    }
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
   * 同じPrefab・設定で`physics: false`を指定し直す。選択状態は画面の
   * チェックに寄せる。
   */
  private appendPhysicsRetry(): void {
    const note = document.createElement("p");
    note.textContent = "Prefabから物理を使わず再変換できます";
    const retry = document.createElement("button");
    retry.type = "button";
    retry.dataset["testid"] = "retry-without-physics";
    retry.textContent = "物理を使わず再変換";
    retry.addEventListener("click", () => {
      // 再変換の指定と画面の選択を一致させ、古い応答で戻らないようにする。
      this.physicsCheck.checked = false;
      void this.convert(false);
    });
    this.errorPanel.append(note, retry);
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
    this.targets = [];
    this.unreadable = [];
    this.selectedPrefab = null;
    this.vrmBytes = null;
    this.viewer?.clear();
    this.candidatesBox.replaceChildren();
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent = "パッケージを開くとPrefab候補が並びます";
    this.candidatesBox.append(hint);
    this.resolvedFbx.textContent = "Prefabを選ぶと対象FBXを表示します";
    this.prefabNote.replaceChildren();
    this.prefabNote.hidden = true;
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

  /** 選択したファイルを開き、Prefab一覧を取り出す。 */
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
      // `open`の戻り値（FBX候補）はPrefab入口では使わない。パッケージの
      // 保持だけを目的に呼び、続けてPrefab一覧を取り出す。
      await client.open(bytes, (stage) => {
        if (this.isCurrent(generation)) {
          this.setStatusText(progressText(stage));
        }
      });
      if (!this.isCurrent(generation)) {
        return;
      }
      this.setStatusText("Prefab一覧を取得しています…");
      const report = await client.prefabTargets((stage) => {
        if (this.isCurrent(generation)) {
          this.setStatusText(progressText(stage));
        }
      });
      if (!this.isCurrent(generation)) {
        return;
      }
      this.onTargets(report);
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

  /** Prefab一覧の受け取り後の分岐。0件・解決なし・1件自動・複数は選択必須に分ける。 */
  private onTargets(report: PrefabTargetsReport): void {
    this.targets = [...report.candidates].sort((a, b) => (a.prefab < b.prefab ? -1 : a.prefab > b.prefab ? 1 : 0));
    this.unreadable = [...report.unreadable];
    this.renderTargets();
    if (this.targets.length === 0) {
      this.setState("error", "Prefabが見つかりませんでした");
      this.showError({
        stage: null,
        message: "Prefabが見つかりませんでした。アバターのPrefabを含む.unitypackageを選んでください",
      });
      return;
    }
    const resolved = resolvedCount(this.targets);
    if (resolved === 0) {
      this.setState("error", "変換可能なPrefabが見つかりませんでした");
      this.showError({ stage: null, message: this.unresolvableMessage() });
      return;
    }
    if (resolved === 1) {
      const selected = initialSelection(this.targets);
      if (selected !== null) {
        // 候補が一意なら選ぶ操作を挟まず自動で選ぶ。変換までは自動で進めない。
        this.selectPrefab(selected);
        return;
      }
    }
    this.setState("selecting", targetsPrompt(this.targets, this.unreadable.length));
  }

  /** 解決できるPrefabが無い理由を代表状態でまとめる。 */
  private unresolvableMessage(): string {
    const counts = new Map<string, number>();
    for (const candidate of this.targets) {
      counts.set(candidate.state, (counts.get(candidate.state) ?? 0) + 1);
    }
    const parts: string[] = [];
    for (const [state, count] of counts) {
      const sample = this.targets.find((candidate) => candidate.state === state);
      const label = sample === undefined ? state : stateLabel(sample);
      parts.push(`${label}（${count}件）`);
    }
    return `変換可能なPrefabが見つかりませんでした（${parts.join("、")}）。ネストPrefabの解決や複数FBX参照は未対応です`;
  }

  /** Prefab候補の一覧（ラジオ）を描く。選べない候補も理由を添えて示す。 */
  private renderTargets(): void {
    this.candidatesBox.replaceChildren();
    for (const candidate of this.targets) {
      const label = document.createElement("label");
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "prefab-target";
      radio.value = candidate.prefab;
      radio.checked = candidate.prefab === this.selectedPrefab;
      const selectable = isSelectable(candidate);
      radio.disabled = !selectable;
      // 処理中の無効化で選べる候補を見分けるための目印。`refreshButtons`で使う。
      radio.dataset["selectable"] = selectable ? "true" : "false";
      if (selectable) {
        radio.addEventListener("change", () => {
          this.selectPrefab(candidate.prefab);
        });
      }
      const pathText = document.createElement("span");
      pathText.textContent = candidate.prefab;
      const stateText = document.createElement("span");
      stateText.className = "candidate-size";
      stateText.textContent = `（${stateLabel(candidate)}）`;
      label.append(radio, pathText, stateText);
      const summary = targetSummary(candidate);
      if (summary !== "") {
        const summaryLine = document.createElement("span");
        summaryLine.className = "hint";
        summaryLine.textContent = summary;
        label.append(summaryLine);
      }
      this.candidatesBox.append(label);
    }
    this.appendPrefabUnreadable();
  }

  /** 読めなかったPrefabの記録を件数と代表で警告する。無い場合は何もしない。 */
  private appendPrefabUnreadable(): void {
    if (this.unreadable.length === 0) {
      return;
    }
    const first = this.unreadable[0];
    const warn = document.createElement("p");
    warn.className = "prefab-warn";
    warn.dataset["testid"] = "prefab-unreadable";
    warn.textContent =
      first === undefined
        ? `読めなかったPrefabが${this.unreadable.length}件あります`
        : `読めなかったPrefabが${this.unreadable.length}件あります（例: ${first.path}: ${first.reason}）`;
    this.candidatesBox.append(warn);
  }

  /**
   * Prefabを選ぶ。対象FBXの表示と作品名の初期値をそろえる。
   *
   * 一覧を作り直すとクリックした要素がDOMから外れてフォーカスが途切れるため、
   * 既存のラジオの印だけを合わせる。ネストPrefabの注意は変換に含まれない
   * 範囲として示す。
   *
   * @param path - 選ぶPrefabの出力パス。
   */
  private selectPrefab(path: string): void {
    if (this.state === "opening" || this.state === "converting") {
      // 処理中の受け付けは世代も表示も変えず、状態文だけで断る。
      // クリックで印だけが動いていた場合は選んだPrefabに戻す。
      this.syncTargetRadios();
      this.setStatusText("処理中はPrefabを変更できません。完了するかキャンセルしてください");
      return;
    }
    const target = this.targets.find((candidate) => candidate.prefab === path) ?? null;
    if (target === null || !isSelectable(target) || target.fbx === null) {
      this.setStatusText("変換するPrefabを選んでください");
      return;
    }
    this.selectedPrefab = path;
    // 選択のたびに一覧を作り直すと、クリックした要素がDOMから外れて
    // フォーカスと操作が途切れる。既存のラジオの印だけを合わせる。
    this.syncTargetRadios();
    this.resolvedFbx.textContent = `対象FBX: ${target.fbx.path}`;
    if (target.nestedPrefabs.length > 0) {
      this.prefabNote.textContent = `ネストPrefabのジオメトリは出力しません: ${target.nestedPrefabs.join(", ")}`;
      this.prefabNote.hidden = false;
    } else {
      this.prefabNote.replaceChildren();
      this.prefabNote.hidden = true;
    }
    // 利用者が作品名を入力済みなら上書きしない。空のときだけ対象FBX名で埋める。
    if (this.nameInput.value.trim() === "") {
      this.nameInput.value = fbxStem(target.fbx.path);
    }
    this.setState("selecting", "設定を確認して「変換」を押してください");
  }

  /**
   * 選んだPrefabで変換する。設定を変えた後の再変換も同じ入口を使う。
   *
   * 未選択・未解決のまま変換しない。物理の失敗からの再変換は
   * `physicsOverride`に`false`を渡し、同じPrefab・設定で物理なしにする。
   *
   * @param physicsOverride - 物理の有無の上書き。省略時は画面のチェックを使う。
   */
  private async convert(physicsOverride?: boolean): Promise<void> {
    const client = this.client;
    const prefabPath = this.selectedPrefab;
    if (client === null || prefabPath === null || this.state === "opening" || this.state === "converting") {
      return;
    }
    const target = this.selectedTarget();
    if (target === null || !isSelectable(target)) {
      this.setStatusText("変換するPrefabを選んでください");
      return;
    }
    const physics = physicsOverride ?? this.physicsCheck.checked;
    // 変換要求ごとに世代を進め、旧変換の往復・プレビュー完了を無効化する。
    // パッケージは開いたまま使い回すため、Workerの破棄は行わない。
    const generation = this.generation + 1;
    this.generation = generation;
    this.setState("converting", progressText("convert"));
    this.clearError();
    try {
      const result = await client.convertPrefab(
        prefabPath,
        {
          name: this.nameInput.value,
          authors: parseAuthors(this.authorsInput.value),
          expressions: this.expressionsCheck.checked,
          baseline: this.baselineCheck.checked,
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
      this.setState("selecting", "変換に失敗しました。設定やPrefabを変えて再変換できます");
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

  /** 除外名を短く並べる。長い場合は先頭数件と残り件数にする。 */
  private excludedNamesText(names: readonly string[]): string {
    if (names.length === 0) {
      return "なし";
    }
    if (names.length <= EXCLUDED_NAMES_LIMIT) {
      return names.join(", ");
    }
    const head = names.slice(0, EXCLUDED_NAMES_LIMIT).join(", ");
    return `${head}ほか${names.length - EXCLUDED_NAMES_LIMIT}件`;
  }

  /** Prefab変換のレポート（計数値と割当・基準姿勢・物理）を表で示す。 */
  private renderReport(report: PrefabConversionReport): void {
    this.reportBox.replaceChildren();
    const table = document.createElement("table");
    table.className = "report-table";
    const rows: ReadonlyArray<readonly [string, string]> = [
      ["対象Prefab", report.prefab],
      ["対象FBX", report.fbx],
      ["ネストPrefab", report.nestedPrefabs.length === 0 ? "なし" : report.nestedPrefabs.join(", ")],
      ["バイト数", formatBytes(report.bytes)],
      ["ノード", String(report.nodes)],
      ["メッシュ", String(report.meshes)],
      ["スキン", String(report.skins)],
      ["材質", String(report.materials)],
      ["画像", String(report.images)],
      ["表情", String(report.expressions)],
      ["割当方式", assignmentModeJa(report.assignment.mode)],
      ["有効メッシュ", String(report.assignment.activeMeshes)],
      ["除外メッシュ", String(report.assignment.excludedMeshes)],
      ["除外名", this.excludedNamesText(report.assignment.excludedNames)],
      ["材質プール", String(report.assignment.materialPool)],
      ["基準姿勢メッシュ", String(report.baseline.meshes)],
      ["基準姿勢Shape", String(report.baseline.shapes)],
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
  private renderWarnings(report: ConversionReport | PrefabConversionReport): void {
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
