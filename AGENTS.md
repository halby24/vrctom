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