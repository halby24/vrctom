# vrctom

VRChat向けアバター(Unity Setup)をVRMに変換するための実験的プロジェクト

# 環境構築、動作ターゲット

miseで管理
Rustを使用
Windows, Mac, Linux, Web対応
ライブラリとして開発し、補助的なCLIをつける

## MVP

MacOSターゲット

1. ファイルシステム抽象化層を用意
2. UnityPackageを展開する
3. 各データやファイルをRustの型で表現する (prefab, mesh, material, texture, shaderなど代表的なもの)
4. 内部でVRMデータを段階的に作る (メッシュ、骨、マテリアル)
5. 最低限のVRMをエクスポートする

# リリース

修正や機能追加が `main` に入ったら、版を上げて npm へ公開するのを標準とする。

- `mise run release patch|minor` で版上げ・タグ・push まで行い、公開は CI が行う
- 0.x の間は、不具合修正なら patch、機能追加や挙動の変更なら minor
- 細かいコミットごとではなく、まとまった変更が入った時点で出す
- 手順の詳細は `crates/vrctom-wasm/PUBLISHING.md`
