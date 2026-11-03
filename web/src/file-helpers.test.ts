/**
 * ファイル名まわりと世代判定の純粋な補助関数の振る舞い。
 *
 * DOMやWebGLに触れず、表示と保存名の写像と古い往復の無視だけを確かめる。
 */
import { describe, expect, it } from "vitest";
import { fbxStem, isCurrentGeneration, parseAuthors, safeFileName } from "./file-helpers.js";

describe("FBXの素朴な名前", () => {
  it("出力パスの末尾から拡張子を除く", () => {
    expect(fbxStem("Assets/Avatar/Body.fbx")).toBe("Body");
  });

  it("区切りがない名前もそのまま扱う", () => {
    expect(fbxStem("Body.fbx")).toBe("Body");
  });

  it("空の名前は仮の名前になる", () => {
    expect(fbxStem("")).toBe("model");
  });
});

describe("作者欄の読み取り", () => {
  it("カンマと読点のどちらでも区切る", () => {
    expect(parseAuthors("ある作者,別の作者、もう一人")).toStrictEqual(["ある作者", "別の作者", "もう一人"]);
  });

  it("空の要素は除く", () => {
    expect(parseAuthors(" ある作者 ,, ")).toStrictEqual(["ある作者"]);
  });
});

describe("安全な保存名", () => {
  it("空の作品名は既定の名前になる", () => {
    expect(safeFileName("   ")).toBe("avatar.vrm");
  });

  it("保存できない文字を除いて拡張子を付ける", () => {
    expect(safeFileName("私のアバター:改訂版")).toBe("私のアバター改訂版.vrm");
  });

  it("すでにある拡張子は重ねない", () => {
    expect(safeFileName("avatar.vrm")).toBe("avatar.vrm");
  });
});

describe("世代の一致判定", () => {
  it("同じ世代は最新の要求として扱う", () => {
    expect(isCurrentGeneration(3, 3)).toBe(true);
  });

  it("古い世代は無視する対象として扱う", () => {
    expect(isCurrentGeneration(4, 3)).toBe(false);
  });
});
