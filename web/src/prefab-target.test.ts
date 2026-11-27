/**
 * Prefab入口の純粋ヘルパーの検証。表示文と選択判断の分岐を確かめる。
 */

import { describe, expect, test } from "vitest";

import type { PrefabTargetCandidate } from "./conversion-client.js";
import {
  initialSelection,
  isSelectable,
  resolvedCount,
  commonDirectory,
  relativeTo,
  splitAssetPath,
  stateLabel,
  targetDetails,
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

  test("解決済みのシーン候補も選択可能としてFBXを参照できる", () => {
    const candidate = makeCandidate({
      prefab: "Assets/ペンギン/ペンギン.unity",
      state: "resolved",
      fbx: { path: "Assets/ペンギン/source/ペンギン_FBX.fbx", guid: "a4bd84bc66f71754582f32c727ccb528" },
    });
    expect(isSelectable(candidate)).toBe(true);
    if (isSelectable(candidate)) {
      expect(candidate.fbx.path).toBe("Assets/ペンギン/source/ペンギン_FBX.fbx");
    }
  });
});

describe("stateLabel", () => {
  test("解決済みは変換可能とだけ示す", () => {
    const candidate = makeCandidate({
      prefab: "Assets/A.prefab",
      state: "resolved",
      fbx: { path: "Assets/A.fbx", guid: "guid-a" },
    });
    expect(stateLabel(candidate)).toBe("変換可能");
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

describe("targetDetails", () => {
  test("対象FBXとネストと未解決を共通ディレクトリからの相対で並べる", () => {
    const candidate = makeCandidate({
      prefab: "Assets/X/A.prefab",
      state: "resolved",
      fbx: { path: "Assets/X/Models/A.fbx", guid: "guid-a" },
      nestedPrefabs: ["Assets/X/Nested.prefab"],
      unresolvedReferences: ["deadbeef"],
    });
    expect(targetDetails(candidate, "Assets/X/")).toEqual([
      { label: "FBX", value: "Models/A.fbx", title: "Assets/X/Models/A.fbx" },
      { label: "ネストPrefab", value: "Nested.prefab", title: "Assets/X/Nested.prefab" },
      { label: "未解決参照", value: "1件", title: "deadbeef" },
    ]);
  });

  test("複数のネストは件数に畳む", () => {
    const candidate = makeCandidate({
      prefab: "Assets/A.unity",
      state: "requiresPrefabResolution",
      nestedPrefabs: ["Assets/B.prefab", "Assets/C.prefab"],
    });
    expect(targetDetails(candidate, "")).toEqual([
      { label: "ネストPrefab", value: "2件", title: "Assets/B.prefab\nAssets/C.prefab" },
    ]);
  });

  test("情報が無い場合は空になる", () => {
    expect(targetDetails(makeCandidate({ prefab: "Assets/N.prefab", state: "noModel" }), "")).toEqual([]);
  });
});

describe("パスの分割と共通ディレクトリ", () => {
  test("ファイル名と親ディレクトリに分ける", () => {
    expect(splitAssetPath("Assets/X/A(PC).prefab")).toEqual({ name: "A(PC).prefab", dir: "Assets/X/" });
    expect(splitAssetPath("A.prefab")).toEqual({ name: "A.prefab", dir: "" });
  });

  test("ディレクトリ単位で共通部分を求める", () => {
    expect(
      commonDirectory(["Assets/WWR/Medusa/Avatars/A.prefab", "Assets/WWR/Medusa/_Resources/Prefabs/B.prefab"]),
    ).toBe("Assets/WWR/Medusa/");
    expect(commonDirectory(["Assets/Ab/A.prefab", "Assets/Ac/B.prefab"])).toBe("Assets/");
    expect(commonDirectory(["A.prefab"])).toBe("");
    expect(commonDirectory([])).toBe("");
  });

  test("共通ディレクトリを除く", () => {
    expect(relativeTo("Assets/X/A.fbx", "Assets/X/")).toBe("A.fbx");
    expect(relativeTo("Other/A.fbx", "Assets/X/")).toBe("Other/A.fbx");
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
