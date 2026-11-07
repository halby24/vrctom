/**
 * Prefab選択の判断ロジックの振る舞い。
 *
 * DOMやWebGLに触れず、1件自動・複数は選択必須・0件はauto継続・
 * unreadableは停止して選択要求・取得失敗は停止の判断だけを確かめる。
 */
import { describe, expect, it } from "vitest";
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

describe("Prefab選択の判断", () => {
  it("1件のみ異常なしは自動選択になる", () => {
    const decision = decidePrefabSelection({
      candidates: ["Assets/A/avatar.prefab"],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(decision).toStrictEqual({
      kind: "auto-prefab",
      prefab: "Assets/A/avatar.prefab",
    });
    expect(selectionFromDecision(decision)).toBe("Assets/A/avatar.prefab");
  });

  it("複数件は選択必須で停止する", () => {
    const decision = decidePrefabSelection({
      candidates: ["Assets/A/avatar.prefab", "Assets/B/avatar.prefab"],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(decision).toStrictEqual({ kind: "pending", reason: "multiple" });
    expect(selectionFromDecision(decision)).toBeNull();
  });

  it("0件で異常なしはautoのまま継続する", () => {
    const decision = decidePrefabSelection({
      candidates: [],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(decision).toStrictEqual({ kind: "auto-undetected" });
    expect(selectionFromDecision(decision)).toBe("auto");
  });

  it("唯一候補でも解析不能があれば停止して選択を要求する", () => {
    const decision = decidePrefabSelection({
      candidates: ["Assets/A/avatar.prefab"],
      unreadableCount: 1,
      fetchFailed: false,
    });
    expect(decision).toStrictEqual({ kind: "pending", reason: "unreadable" });
    expect(selectionFromDecision(decision)).toBeNull();
  });

  it("0件でも解析不能があれば自動継続せず停止する", () => {
    const decision = decidePrefabSelection({
      candidates: [],
      unreadableCount: 1,
      fetchFailed: false,
    });
    expect(decision).toStrictEqual({ kind: "pending", reason: "unreadable" });
    expect(selectionFromDecision(decision)).toBeNull();
  });

  it("取得失敗は候補の有無によらず停止する", () => {
    for (const candidates of [[], ["Assets/A/avatar.prefab"]]) {
      const decision = decidePrefabSelection({
        candidates,
        unreadableCount: 0,
        fetchFailed: true,
      });
      expect(decision).toStrictEqual({ kind: "pending", reason: "fetch-failed" });
      expect(selectionFromDecision(decision)).toBeNull();
    }
  });
});

describe("選択状態から変換指定への写し", () => {
  it("1件の自動選択はautoのまま渡す", () => {
    const decision = decidePrefabSelection({
      candidates: ["Assets/A/avatar.prefab"],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(physicsOptionFromSelection("Assets/A/avatar.prefab", decision)).toBe("auto");
  });

  it("利用者が選び直した場合は明示の指定になる", () => {
    const decision = decidePrefabSelection({
      candidates: ["Assets/A/avatar.prefab", "Assets/B/avatar.prefab"],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(physicsOptionFromSelection("Assets/B/avatar.prefab", decision)).toStrictEqual({
      prefab: "Assets/B/avatar.prefab",
    });
  });

  it("未選択は変換を待たせるため空を返す", () => {
    const decision = decidePrefabSelection({
      candidates: ["Assets/A/avatar.prefab", "Assets/B/avatar.prefab"],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(physicsOptionFromSelection(null, decision)).toBeNull();
  });

  it("物理なしと0件継続の指定はそのまま渡す", () => {
    const decision = decidePrefabSelection({
      candidates: [],
      unreadableCount: 0,
      fetchFailed: false,
    });
    expect(physicsOptionFromSelection("none", decision)).toBe("none");
    expect(physicsOptionFromSelection("auto", decision)).toBe("auto");
  });
});

describe("変換の可否", () => {
  it("取得中と未選択のまま変換しない", () => {
    expect(isPrefabReady(null, false)).toBe(false);
    expect(isPrefabReady("auto", true)).toBe(false);
    expect(isPrefabReady(null, true)).toBe(false);
  });

  it("自動継続と明示選択は変換できる", () => {
    expect(isPrefabReady("auto", false)).toBe(true);
    expect(isPrefabReady("Assets/A/avatar.prefab", false)).toBe(true);
    expect(isPrefabReady("none", false)).toBe(true);
  });
});

describe("促し文の決定", () => {
  it("未選択で複数件は候補件数と物理なしの案内になる", () => {
    expect(
      prefabPromptText({
        candidates: ["Assets/A/avatar.prefab", "Assets/Milfy.prefab"],
        unreadableCount: 0,
        fetchFailed: false,
        selection: null,
      }),
    ).toBe(
      "使うPrefabを選んでください（候補2件）。物理を使わない場合は「使わない」を選んでください",
    );
  });

  it("未選択で読めなかったPrefabがあれば自動で進めない旨になる", () => {
    expect(
      prefabPromptText({
        candidates: ["Assets/A/avatar.prefab", "Assets/Milfy.prefab"],
        unreadableCount: 1,
        fetchFailed: false,
        selection: null,
      }),
    ).toBe(
      "読めなかったPrefabがあるため自動で進めません。使うPrefabを選んでください（候補2件）。物理を使わない場合は「使わない」を選んでください",
    );
  });

  it("未選択で取得失敗は停止と物理なし続行の説明になる", () => {
    expect(
      prefabPromptText({
        candidates: [],
        unreadableCount: 0,
        fetchFailed: true,
        selection: null,
      }),
    ).toBe(
      "Prefab候補の取得に失敗しました。「Prefab由来の物理を使わず続行」を選ぶと、物理・T-poseの適用・視線（lookAt）を省略して出力します",
    );
  });

  it("明示Prefabの選択は選んだパスをそのまま案内する", () => {
    expect(
      prefabPromptText({
        candidates: ["Assets/A/avatar.prefab", "Assets/Milfy.prefab"],
        unreadableCount: 0,
        fetchFailed: false,
        selection: "Assets/Milfy.prefab",
      }),
    ).toBe("使用Prefab: Assets/Milfy.prefab。設定を確認して「変換」を押してください");
  });

  it("物理なしの選択は取得失敗後も停止文言を残さない", () => {
    for (const fetchFailed of [false, true]) {
      const text = prefabPromptText({
        candidates: fetchFailed ? [] : ["Assets/A/avatar.prefab", "Assets/Milfy.prefab"],
        unreadableCount: 0,
        fetchFailed,
        selection: "none",
      });
      expect(text).toBe(
        "Prefab由来の物理を使わず出力します。設定を確認して「変換」を押してください",
      );
      expect(text).not.toContain("停止");
    }
  });

  it("autoの選択は対応Prefabなしで物理なしに出す旨になる", () => {
    expect(
      prefabPromptText({
        candidates: [],
        unreadableCount: 0,
        fetchFailed: false,
        selection: "auto",
      }),
    ).toBe(
      "対応Prefabを検出できないため物理なしで出力します。設定を確認して「変換」を押してください",
    );
  });
});

describe("枠内表示の決定", () => {
  it("取得失敗の未選択は停止し物理なしを選ぶと継続に変わる", () => {
    const pending = prefabBoxState({
      candidates: [],
      unreadableCount: 0,
      fetchFailed: true,
      selection: null,
      loading: false,
      fetchError: "network error",
    });
    expect(pending).toStrictEqual({
      kind: "fetch-failed-pending",
      error: "network error",
    });
    const continued = prefabBoxState({
      candidates: [],
      unreadableCount: 0,
      fetchFailed: true,
      selection: "none",
      loading: false,
      fetchError: "network error",
    });
    expect(continued).toStrictEqual({
      kind: "fetch-failed-continued",
      error: "network error",
    });
  });

  it("取得失敗から物理なしを選ぶと停止文言が消え失敗理由は残る", () => {
    const pendingStatus = prefabBoxStatusText({
      candidates: [],
      unreadableCount: 0,
      fetchFailed: true,
      selection: null,
      loading: false,
      fetchError: "network error",
    });
    expect(pendingStatus).toContain("停止しました");
    const continuedStatus = prefabBoxStatusText({
      candidates: [],
      unreadableCount: 0,
      fetchFailed: true,
      selection: "none",
      loading: false,
      fetchError: "network error",
    });
    expect(continuedStatus).not.toContain("停止");
    expect(continuedStatus).toContain("Prefab由来の物理を使わず出力します");
    expect(prefabFetchContinuedText("network error")).not.toContain("停止");
    expect(prefabFetchContinuedText("network error")).toContain("network error");
    expect(prefabFetchFailedWarnText("network error")).toContain("停止しました");
  });

  it("明示Prefabの選択は選択肢の一覧になり状態行も選んだパスになる", () => {
    const state = prefabBoxState({
      candidates: ["Assets/A/avatar.prefab", "Assets/Milfy.prefab"],
      unreadableCount: 0,
      fetchFailed: false,
      selection: "Assets/Milfy.prefab",
      loading: false,
      fetchError: null,
    });
    expect(state).toStrictEqual({ kind: "select", reason: "multiple" });
    expect(
      prefabBoxStatusText({
        candidates: ["Assets/A/avatar.prefab", "Assets/Milfy.prefab"],
        unreadableCount: 0,
        fetchFailed: false,
        selection: "Assets/Milfy.prefab",
        loading: false,
        fetchError: null,
      }),
    ).toBe("使用Prefab: Assets/Milfy.prefab。設定を確認して「変換」を押してください");
  });
});
