/**
 * 境界の実行時検証の振る舞い。Worker・wasm由来のJSONを想定する。
 *
 * DOMやWebGLに触れず、純粋な検証だけを確かめる。
 */
import { describe, expect, it } from "vitest";
import {
  ConversionError,
  isMainToWorkerMessage,
  parseConversionReport,
  parseFbxCandidates,
} from "./conversion-client.js";

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
    };
  }

  it("計数値と警告文の組をそのまま通す", () => {
    const report = parseConversionReport(validReport());
    expect(report.fbx).toBe("Assets/A/avatar.fbx");
    expect(report.warnings).toStrictEqual(["見た目を確認すること"]);
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
});
