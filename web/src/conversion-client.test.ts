/**
 * 境界の実行時検証の振る舞い。Worker・wasm由来のJSONを想定する。
 *
 * DOMやWebGLに触れず、純粋な検証だけを確かめる。
 */
import { describe, expect, it } from "vitest";
import {
  ConversionClient,
  ConversionError,
  isMainToWorkerMessage,
  parseClientConversionReport,
  parseConversionReport,
  parseFbxCandidates,
  parsePrefabCandidatesReport,
  parsePhysicsReport,
} from "./conversion-client.js";
import type { ConversionReport } from "./conversion-client.js";

describe("FBX候補の検証", () => {
  it("パスと大きさの組をそのまま通す", () => {
    const candidates = parseFbxCandidates([
      { path: "Assets/A/avatar.fbx", size: 10 },
      { path: "Assets/B/avatar.fbx", size: 20 },
    ]);
    expect(candidates).toStrictEqual([
      { path: "Assets/A/avatar.fbx", size: 10 },
      { path: "Assets/B/avatar.fbx", size: 20 },
    ]);
  });

  it("空の一覧は見つからなかった場合の表示に使うため通す", () => {
    expect(parseFbxCandidates([])).toStrictEqual([]);
  });

  it("一覧でない値は候補の読み取り失敗になる", () => {
    expect(() => parseFbxCandidates({})).toThrow(ConversionError);
  });

  it("パスが空の項目は候補の読み取り失敗になる", () => {
    expect(() => parseFbxCandidates([{ path: "", size: 10 }])).toThrow(ConversionError);
  });

  it("大きさが負の項目は候補の読み取り失敗になる", () => {
    expect(() => parseFbxCandidates([{ path: "Assets/A/avatar.fbx", size: -1 }])).toThrow(ConversionError);
  });
});

describe("レポートの検証", () => {
  /** 検査用の正当な物理報告。項目ごとに崩して失敗を確かめる。 */
  function validPhysics(): Record<string, unknown> {
    return {
      prefab: "Assets/A/avatar.prefab",
      selection: "auto",
      springs: 2,
      joints: 6,
      colliders: 3,
      groups: 1,
      tpose_applied: 4,
      tpose_skipped: 1,
      look_at: "bone",
    };
  }

  /** 検査用の正当なレポート。項目ごとに崩して失敗を確かめる。 */
  function validReport(): Record<string, unknown> {
    return {
      fbx: "Assets/A/avatar.fbx",
      bytes: 100,
      nodes: 1,
      meshes: 1,
      skins: 0,
      materials: 1,
      images: 1,
      expressions: 0,
      warnings: ["見た目を確認すること"],
      physics: validPhysics(),
    };
  }

  it("計数値と警告文の組をそのまま通す", () => {
    const report = parseConversionReport(validReport());
    expect(report.fbx).toBe("Assets/A/avatar.fbx");
    expect(report.warnings).toStrictEqual(["見た目を確認すること"]);
    expect(report.physics.prefab).toBe("Assets/A/avatar.prefab");
    expect(report.physics.selection).toBe("auto");
  });

  it("項目が欠けた値はレポートの読み取り失敗になる", () => {
    const broken = validReport();
    delete broken["meshes"];
    expect(() => parseConversionReport(broken)).toThrow(ConversionError);
  });

  it("警告文に文字列でない混じりがあるとレポートの読み取り失敗になる", () => {
    const broken = validReport();
    broken["warnings"] = ["正当な警告", 42];
    expect(() => parseConversionReport(broken)).toThrow(ConversionError);
  });

  it("物理の項目が欠けた値はレポートの読み取り失敗になる", () => {
    const broken = validReport();
    delete broken["physics"];
    expect(() => parseConversionReport(broken)).toThrow(ConversionError);
  });

  it("物理なしの報告は空の件数とnullで通す", () => {
    const report = parseConversionReport({
      ...validReport(),
      physics: {
        prefab: null,
        selection: "none",
        springs: 0,
        joints: 0,
        colliders: 0,
        groups: 0,
        tpose_applied: 0,
        tpose_skipped: 0,
        look_at: null,
      },
    });
    expect(report.physics.prefab).toBeNull();
    expect(report.physics.selection).toBe("none");
    expect(report.physics.lookAt).toBeNull();
  });

  it("物理の選択理由が崩れているとレポートの読み取り失敗になる", () => {
    const broken = validReport();
    broken["physics"] = { ...validPhysics(), selection: "sometimes" };
    expect(() => parseConversionReport(broken)).toThrow(ConversionError);
  });

  it("物理の視線の種別が崩れているとレポートの読み取り失敗になる", () => {
    const broken = validReport();
    broken["physics"] = { ...validPhysics(), look_at: "head" };
    expect(() => parseConversionReport(broken)).toThrow(ConversionError);
  });

  it("物理の件数が負だとレポートの読み取り失敗になる", () => {
    expect(() => parsePhysicsReport({ ...validPhysics(), springs: -1 })).toThrow(ConversionError);
  });
});

describe("Workerからclientへの往復", () => {
  /**
   * Workerが送る形式（`camelCase`済み・`physics`込み）の応答をclientの受信処理に通す。
   *
   * 構造化複製で`report`は複製される想定であり、JSONで写して受信値にする。
   * `ConversionClient`の待ち合わせ解決までを通し、成功してレポートが取得できることを確かめる。
   */
  async function 受信処理を通す(wasm形: Record<string, unknown>): Promise<ConversionReport> {
    // Worker側はwasm境界の検証で`camelCase`へ写してから送る。
    const worker済み = parseConversionReport(wasm形);
    const stub = { postMessage(): void {}, terminate(): void {} } as unknown as Worker;
    const client = new ConversionClient(stub);
    try {
      const pending = client.convert("Assets/A/avatar.fbx", { name: "", authors: [], expressions: false });
      // 最初の要求の`id`は1であり、応答も同じ`id`で返す。
      const data: unknown = {
        type: "converted",
        id: 1,
        vrm: new ArrayBuffer(8),
        report: JSON.parse(JSON.stringify(worker済み)),
      };
      const onmessage = (stub as unknown as { onmessage?: (event: { data: unknown }) => void }).onmessage;
      onmessage?.({ data });
      const result = await pending;
      return result.report;
    } finally {
      client.terminate();
    }
  }

  it("物理ありの成功応答を受信処理で取得できる", async () => {
    const report = await 受信処理を通す({
      fbx: "Assets/A/avatar.fbx",
      bytes: 100,
      nodes: 1,
      meshes: 1,
      skins: 0,
      materials: 1,
      images: 1,
      expressions: 0,
      warnings: [],
      physics: {
        prefab: "Assets/A/avatar.prefab",
        selection: "auto",
        springs: 2,
        joints: 6,
        colliders: 3,
        groups: 1,
        tpose_applied: 4,
        tpose_skipped: 1,
        look_at: "bone",
      },
    });
    expect(report.physics.prefab).toBe("Assets/A/avatar.prefab");
    expect(report.physics.selection).toBe("auto");
    expect(report.physics.tposeApplied).toBe(4);
    expect(report.physics.tposeSkipped).toBe(1);
    expect(report.physics.lookAt).toBe("bone");
  });

  it("物理なしの成功応答を受信処理で取得できる", async () => {
    const report = await 受信処理を通す({
      fbx: "Assets/A/avatar.fbx",
      bytes: 100,
      nodes: 1,
      meshes: 1,
      skins: 0,
      materials: 1,
      images: 1,
      expressions: 0,
      warnings: ["対応Prefabを検出できないためPrefab由来の物理なし"],
      physics: {
        prefab: null,
        selection: "undetected",
        springs: 0,
        joints: 0,
        colliders: 0,
        groups: 0,
        tpose_applied: 0,
        tpose_skipped: 0,
        look_at: null,
      },
    });
    expect(report.physics.prefab).toBeNull();
    expect(report.physics.selection).toBe("undetected");
    expect(report.physics.lookAt).toBeNull();
  });

  it("client境界の検証はwasm境界のsnake_caseを受け付けない", () => {
    // 二つの境界の検証を混同すると成功応答が失敗になるため、境界の違いを固定する。
    const wasm形: Record<string, unknown> = {
      fbx: "Assets/A/avatar.fbx",
      bytes: 100,
      nodes: 1,
      meshes: 1,
      skins: 0,
      materials: 1,
      images: 1,
      expressions: 0,
      warnings: [],
      physics: {
        prefab: "Assets/A/avatar.prefab",
        selection: "auto",
        springs: 2,
        joints: 6,
        colliders: 3,
        groups: 1,
        tpose_applied: 4,
        tpose_skipped: 1,
        look_at: "bone",
      },
    };
    expect(() => parseClientConversionReport(wasm形)).toThrow(ConversionError);
  });
});

describe("Prefab候補の検証", () => {
  /** 検査用の正当な候補報告。項目ごとに崩して失敗を確かめる。 */
  function validPrefabReport(): Record<string, unknown> {
    return {
      fbx: "Assets/A/avatar.fbx",
      guid: "0123456789abcdef0123456789abcdef",
      candidates: ["Assets/A/avatar.prefab"],
      unreadable: [{ path: "Assets/B/broken.prefab", reason: "Prefabの解析に失敗した" }],
    };
  }

  it("候補と読めなかった記録の組をそのまま通す", () => {
    const report = parsePrefabCandidatesReport(validPrefabReport());
    expect(report.fbx).toBe("Assets/A/avatar.fbx");
    expect(report.candidates).toStrictEqual(["Assets/A/avatar.prefab"]);
    expect(report.unreadable).toStrictEqual([
      { path: "Assets/B/broken.prefab", reason: "Prefabの解析に失敗した" },
    ]);
  });

  it("空の候補と空の読めなかった記録を通す", () => {
    const report = parsePrefabCandidatesReport({
      fbx: "Assets/A/avatar.fbx",
      guid: "0123456789abcdef0123456789abcdef",
      candidates: [],
      unreadable: [],
    });
    expect(report.candidates).toStrictEqual([]);
    expect(report.unreadable).toStrictEqual([]);
  });

  it("候補の項目が欠けた値は候補の読み取り失敗になる", () => {
    const broken = validPrefabReport();
    delete broken["candidates"];
    expect(() => parsePrefabCandidatesReport(broken)).toThrow(ConversionError);
  });

  it("guidが欠けた値は候補の読み取り失敗になる", () => {
    const broken = validPrefabReport();
    delete broken["guid"];
    expect(() => parsePrefabCandidatesReport(broken)).toThrow(ConversionError);
  });

  it("候補に空のパスが混じると候補の読み取り失敗になる", () => {
    const broken = validPrefabReport();
    broken["candidates"] = ["Assets/A/avatar.prefab", ""];
    expect(() => parsePrefabCandidatesReport(broken)).toThrow(ConversionError);
  });

  it("読めなかった記録の形が崩れると候補の読み取り失敗になる", () => {
    const broken = validPrefabReport();
    broken["unreadable"] = [{ path: "Assets/B/broken.prefab" }];
    expect(() => parsePrefabCandidatesReport(broken)).toThrow(ConversionError);
  });

  it("一覧でない値は候補の読み取り失敗になる", () => {
    expect(() => parsePrefabCandidatesReport([])).toThrow(ConversionError);
  });
});

describe("mainからWorkerへの要求の検証", () => {
  it("開く要求は転送済みのバイト列とともに通す", () => {
    expect(isMainToWorkerMessage({ type: "open", id: 1, bytes: new ArrayBuffer(8) })).toBe(true);
  });

  it("変換要求は設定の組とともに通す", () => {
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 2,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: ["ある作者"], expressions: true },
      }),
    ).toBe(true);
  });

  it("中断要求はそのまま通す", () => {
    expect(isMainToWorkerMessage({ type: "cancel", id: 3 })).toBe(true);
  });

  it("種別が未知の値は要求として扱わない", () => {
    expect(isMainToWorkerMessage({ type: "save", id: 4 })).toBe(false);
  });

  it("変換要求で作者欄が文字列でないと要求として扱わない", () => {
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 5,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: ["正当な作者", 42], expressions: false },
      }),
    ).toBe(false);
  });

  it("変換要求は物理の自動・なし・明示とともに通す", () => {
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 6,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: [], expressions: false, physics: "auto" },
      }),
    ).toBe(true);
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 7,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: [], expressions: false, physics: "none" },
      }),
    ).toBe(true);
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 8,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: [], expressions: false, physics: { prefab: "Assets/A/avatar.prefab" } },
      }),
    ).toBe(true);
  });

  it("変換要求で物理の形が崩れていると要求として扱わない", () => {
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 9,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: [], expressions: false, physics: "sometimes" },
      }),
    ).toBe(false);
    expect(
      isMainToWorkerMessage({
        type: "convert",
        id: 10,
        fbxPath: "Assets/A/avatar.fbx",
        options: { name: "avatar", authors: [], expressions: false, physics: { prefab: "" } },
      }),
    ).toBe(false);
  });

  it("Prefab候補の取得要求は対象FBXとともに通す", () => {
    expect(isMainToWorkerMessage({ type: "prefabCandidates", id: 11, fbxPath: "Assets/A/avatar.fbx" })).toBe(true);
  });

  it("Prefab候補の取得要求でパスが空だと要求として扱わない", () => {
    expect(isMainToWorkerMessage({ type: "prefabCandidates", id: 12, fbxPath: "" })).toBe(false);
  });
});
