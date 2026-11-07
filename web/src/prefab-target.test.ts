/**
 * Prefab入口の純粋ヘルパーの検証。表示文と選択判断の分岐を確かめる。
 */

import { describe, expect, test } from "vitest";

import type { PrefabTargetCandidate } from "./conversion-client.js";
import {
  initialSelection,
  isSelectable,
  resolvedCount,
  stateLabel,
  targetSummary,
  targetsPrompt,
} from "./prefab-target.js";

/** テスト用の候補を組み立てる。指定以外の列は空にする。 */
function makeCandidate(partial: Partial<PrefabTargetCandidate> & { prefab: string }): PrefabTargetCandidate {
  return {
    state: "resolved",
    fbx: null,
    nestedPrefabs: [],
    otherReferences: [],
    unresolvedReferences: [],
    ...partial,
  };
}

describe("isSelectable", () => {
  test("解決済みでFBXがある場合だけ選べる", () => {
    const selectable = makeCandidate({
      prefab: "Assets/A.prefab",
      state: "resolved",
      fbx: { path: "Assets/A.fbx", guid: "guid-a" },
    });
    expect(isSelectable(selectable)).toBe(true);
  });

  test("解決済みでもFBXが無い場合は選べない", () => {
    const missing = makeCandidate({ prefab: "Assets/B.prefab", state: "resolved", fbx: null });
    expect(isSelectable(missing)).toBe(false);
  });

  test("未解決の状態は選べない", () => {
    for (const state of ["noModel", "unsupportedMultipleFbx", "requiresPrefabResolution", "unresolved"] as const) {
      expect(isSelectable(makeCandidate({ prefab: `Assets/${state}.prefab`, state }))).toBe(false);
    }
  });
});

describe("stateLabel", () => {
  test("解決済みは対象FBXを添える", () => {
    const candidate = makeCandidate({
      prefab: "Assets/A.prefab",
      state: "resolved",
      fbx: { path: "Assets/A.fbx", guid: "guid-a" },
    });
    expect(stateLabel(candidate)).toBe("変換可能（対象FBX: Assets/A.fbx）");
  });

  test("状態ごとに日本語文を返す", () => {
    expect(stateLabel(makeCandidate({ prefab: "Assets/N.prefab", state: "noModel" }))).toBe("FBX参照がない");
    expect(
      stateLabel(makeCandidate({ prefab: "Assets/M.prefab", state: "unsupportedMultipleFbx" })),
    ).toBe("直接参照するFBXが複数あるため未対応");
    expect(
      stateLabel(makeCandidate({ prefab: "Assets/R.prefab", state: "requiresPrefabResolution" })),
    ).toBe("ネストPrefabの解決が必要（未対応）");
    expect(stateLabel(makeCandidate({ prefab: "Assets/U.prefab", state: "unresolved" }))).toBe(
      "参照先を解決できない",
    );
  });
});

describe("targetSummary", () => {
  test("対象FBXとネストと未解決を要約する", () => {
    const candidate = makeCandidate({
      prefab: "Assets/A.prefab",
      state: "resolved",
      fbx: { path: "Assets/A.fbx", guid: "guid-a" },
      nestedPrefabs: ["Assets/Nested.prefab"],
      unresolvedReferences: ["deadbeef"],
    });
    expect(targetSummary(candidate)).toBe(
      "対象FBX: Assets/A.fbx / ネストPrefab（ジオメトリは出力しない）: Assets/Nested.prefab / 未解決参照1件（例: deadbeef）",
    );
  });

  test("情報が無い場合は空文字になる", () => {
    expect(targetSummary(makeCandidate({ prefab: "Assets/N.prefab", state: "noModel" }))).toBe("");
  });
});

describe("resolvedCountとinitialSelection", () => {
  test("解決済みが1件だけのときだけ自動選択する", () => {
    const candidates = [
      makeCandidate({
        prefab: "Assets/A.prefab",
        state: "resolved",
        fbx: { path: "Assets/A.fbx", guid: "guid-a" },
      }),
      makeCandidate({ prefab: "Assets/N.prefab", state: "noModel" }),
    ];
    expect(resolvedCount(candidates)).toBe(1);
    expect(initialSelection(candidates)).toBe("Assets/A.prefab");
  });

  test("複数件のときは自動選択しない", () => {
    const candidates = [
      makeCandidate({
        prefab: "Assets/A.prefab",
        state: "resolved",
        fbx: { path: "Assets/A.fbx", guid: "guid-a" },
      }),
      makeCandidate({
        prefab: "Assets/B.prefab",
        state: "resolved",
        fbx: { path: "Assets/B.fbx", guid: "guid-b" },
      }),
    ];
    expect(resolvedCount(candidates)).toBe(2);
    expect(initialSelection(candidates)).toBeNull();
  });

  test("解決済みが無いときは自動選択しない", () => {
    const candidates = [makeCandidate({ prefab: "Assets/N.prefab", state: "noModel" })];
    expect(resolvedCount(candidates)).toBe(0);
    expect(initialSelection(candidates)).toBeNull();
  });
});

describe("targetsPrompt", () => {
  test("複数件は選択を促す", () => {
    const candidates = [
      makeCandidate({
        prefab: "Assets/A.prefab",
        state: "resolved",
        fbx: { path: "Assets/A.fbx", guid: "guid-a" },
      }),
      makeCandidate({
        prefab: "Assets/B.prefab",
        state: "resolved",
        fbx: { path: "Assets/B.fbx", guid: "guid-b" },
      }),
    ];
    expect(targetsPrompt(candidates, 0)).toContain("変換するPrefabを選んでください");
  });

  test("0件は見つからない旨を返す", () => {
    expect(targetsPrompt([makeCandidate({ prefab: "Assets/N.prefab", state: "noModel" })], 0)).toBe(
      "変換可能なPrefabが見つかりませんでした",
    );
  });
});
