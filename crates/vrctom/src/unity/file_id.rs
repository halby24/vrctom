//! Unity 2019.3 以降のFBX取り込みが生成する fileID。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! 2019.3 以降、FBXから作るサブアセットの fileID は XXH64（seed 0）で作る。
//! 対象文字列は種類で決まる。メッシュは所有モデルの名前だけから作り、
//! 階層は含まない。ゲームオブジェクトとコンポーネントはモデル階層の
//! パス（`/` 区切り）から作る。過去方式（`.meta` の `fileIDToRecycleName`）は
//! 扱わず、解決できない場合は呼び出し側で失敗させる。

use super::FileId;

/// メッシュのサブアセット fileID を作る。
///
/// 対象文字列は `Type:Mesh->{モデル名}0` である。モデル名はFBXのModel名であり、
/// glTFメッシュ名（ジオメトリ名）とは一致しない。頂点数が多い場合の
/// `{名前}_MeshPart{X}` 形は数え方がFBXだけでは定まらないため作らない。
pub fn mesh_file_id(model_name: &str) -> FileId {
    import_file_id(&format!("Type:Mesh->{model_name}0"))
}

/// ゲームオブジェクトのサブアセット fileID を作る。
///
/// 対象文字列は `Type:GameObject->//RootNode/root/{階層パス}0` である。
/// 階層パスはFBXのModel階層を `/` で繋いだものである。
/// 空の場合は取り込みルート自身を指し、対象文字列は区切り無しの
/// `Type:GameObject->//RootNode/root0` になる。資産に依らず同値である。
pub fn game_object_file_id(hierarchy_path: &str) -> FileId {
    if hierarchy_path.is_empty() {
        import_file_id("Type:GameObject->//RootNode/root0")
    } else {
        import_file_id(&format!(
            "Type:GameObject->//RootNode/root/{hierarchy_path}0"
        ))
    }
}

/// スキンドメッシュ描画設定のサブアセット fileID を作る。
///
/// 対象文字列は
/// `Type:SkinnedMeshRenderer->//RootNode/root/{階層パス}/SkinnedMeshRenderer0`
/// である。階層パスは所有モデルのものである。
pub fn skinned_mesh_renderer_file_id(hierarchy_path: &str) -> FileId {
    import_file_id(&format!(
        "Type:SkinnedMeshRenderer->//RootNode/root/{hierarchy_path}/SkinnedMeshRenderer0"
    ))
}

/// 取り込み fileID の本体。XXH64（seed 0）を符号付き64bitへ写す。
///
/// UnityのYAML送出は符号付き10進であり、負値もそのまま比べる。
/// `u64` から `i64` への写像はビットを保つ（ラップアラウンド）ものである。
fn import_file_id(seed_text: &str) -> FileId {
    xxhash_rust::xxh64::xxh64(seed_text.as_bytes(), 0) as FileId
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空文字は既知ベクタと一致する() {
        // XXH64 の仕様値であり、依存の取り違えを検出する。
        assert_eq!(xxhash_rust::xxh64::xxh64(b"", 0), 0xEF46DB3751D8E999);
    }

    #[test]
    fn メッシュ名だけから実データの値を再現する() {
        // マヌカ・マナカ系と無関係に同名メッシュは同値になる。
        // 実データ（MANUKA.fbx 由来・sidecar の fileID）との突き合わせである。
        assert_eq!(mesh_file_id("Body"), 2959467708922500008);
    }

    #[test]
    fn 描画設定とゲームオブジェクトは階層パスから実データの値を再現する() {
        // `MANUKA_lilToon.prefab` の `m_Modifications` の target と一致する。
        // ゲームオブジェクト2件は `m_IsActive: 0` の対象である。
        assert_eq!(skinned_mesh_renderer_file_id("Body"), 1630794972795428178);
        assert_eq!(game_object_file_id("Body"), 635547659545568360);
        assert_eq!(
            game_object_file_id("Manuka_underwear_bra"),
            -1975330403214633973
        );
        assert_eq!(
            game_object_file_id("Manuka_underwear_panty"),
            -95492197830400972
        );
    }

    #[test]
    fn シフォンの無効対象はブラのゲームオブジェクトと一致する() {
        // `Chiffon.prefab` の `m_IsActive: 0` の target と一致する。
        assert_eq!(game_object_file_id("Bra"), -7296742891649885728);
    }

    #[test]
    fn 空パスは資産に依らない取り込みルートと一致する() {
        // `Type:GameObject->//RootNode/root0` の値であり、マヌカ・シフォンの
        // 両Prefabで `m_IsActive`・`m_Name` の対象として現れる固定値である。
        assert_eq!(game_object_file_id(""), 919132149155446097);
    }
}
