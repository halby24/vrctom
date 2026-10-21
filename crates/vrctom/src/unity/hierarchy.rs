//! 単一SerializedFile内の階層の組み立て。
//!
//! 公開契約の入口は [`crate::unity`] を参照。
//!
//! TransformをfileIDで索引し、GameObject対応・親子の整合性を検証して
//! 所有権のある木として返す。`stripped` や `PrefabInstance` を含む場合は
//! 部分木を完全な階層として返さず、明示的な非対応として失敗する。
//!
//! 組み立て・集計・破棄・複写はいずれも反復処理であり、
//! 深い鎖でも呼び出し側のstackを使い果たさない。

use std::collections::{HashMap, HashSet};

use super::{
    FileId, ObjectRef, SerializedFile, Transform, TypedObject, UnityError, UnityErrorKind,
};

/// 階層の深さの既定上限（rootを0とする辺の数）。
///
/// 反復組み立てであり深さ自体でstackを使わないため、上限は暴走への歯止めである。
/// 既定は文書数の上限（100,000）と同規模にし、正常な数千節点の鎖を通す。
const DEFAULT_MAX_DEPTH: usize = 100_000;

/// 階層組み立ての資源上限。
///
/// フィールドは非公開で、既定値は [`Default`] が設定する。
/// 上限の変更は `with_` で始まるメソッドで行う。
#[derive(Debug, Clone)]
pub struct HierarchyOptions {
    max_depth: usize,
}

impl HierarchyOptions {
    /// 既定の上限を持つ設定を生成する。
    pub fn new() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
        }
    }

    /// 階層の深さの上限（rootを0とする辺の数）を変更する。
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }
}

impl Default for HierarchyOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// 単一ファイル内の階層。
///
/// rootは複数あり得る。順序はファイル内の出現順である。
/// 子の順序は `m_Children` のまま保つ。
#[derive(Clone)]
pub struct LocalHierarchy {
    roots: Vec<LocalNode>,
}

impl std::fmt::Debug for LocalHierarchy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 明示スタックで書く。`LocalNode` の表示自体が反復であるため、
        // root列の反復だけを行い、再帰を使わない。
        // 出力形は変えず、`{:#?}` でも同じ簡潔形で書く。
        f.write_str("LocalHierarchy { roots: [")?;
        for (index, root) in self.roots.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            std::fmt::Debug::fmt(root, f)?;
        }
        f.write_str("] }")
    }
}

impl LocalHierarchy {
    /// rootの一覧を返す。
    pub fn roots(&self) -> &[LocalNode] {
        &self.roots
    }

    /// 全節点数を返す。
    pub fn len(&self) -> usize {
        // 再帰にせず数える。深い鎖でもstackを使い果たさない。
        let mut total = 0;
        let mut stack: Vec<&LocalNode> = self.roots.iter().collect();
        while let Some(node) = stack.pop() {
            total += 1;
            stack.extend(node.children.iter());
        }
        total
    }

    /// 節点がないかどうかを返す。
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

/// 階層の1節点。
pub struct LocalNode {
    /// ゲームオブジェクト名。
    pub name: String,
    /// ゲームオブジェクトのfileID。
    pub game_object: FileId,
    /// TransformのfileID。
    pub transform: FileId,
    /// 子節点。順序は `m_Children` のまま保つ。
    pub children: Vec<LocalNode>,
}

impl std::fmt::Debug for LocalNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 所有木の表示を再帰にしない。深い鎖でも呼び出し側のstackを使い果たさない。
        // 開閉の対応を明示スタックで保ち、子順序のまま書く。
        // `{:#?}` でも同じ簡潔形にし、整形のための再帰を入れない。
        f.write_str("LocalNode { name: ")?;
        std::fmt::Debug::fmt(&self.name, f)?;
        f.write_str(", game_object: ")?;
        std::fmt::Debug::fmt(&self.game_object, f)?;
        f.write_str(", transform: ")?;
        std::fmt::Debug::fmt(&self.transform, f)?;
        f.write_str(", children: [")?;
        // `(節点, 次に書く子の位置)` の明示スタックである。
        // 参照は `&self` 由来であり、表示中は木を変えない。
        let mut stack: Vec<(&LocalNode, usize)> = vec![(self, 0)];
        // 対応する深さで開いた子の数を数え、2件目以降の前に区切りを入れる。
        let mut opened: Vec<usize> = vec![0];
        while !stack.is_empty() {
            // 参照と位置は複写して借用を終える。子への参照は `&self` 由来である。
            let (node, next) = *stack.last().expect("空でない");
            if next < node.children.len() {
                let child: &LocalNode = &node.children[next];
                stack.last_mut().expect("空でない").1 += 1;
                let count = opened.last_mut().expect("対応する計数がある");
                *count += 1;
                if *count > 1 {
                    f.write_str(", ")?;
                }
                f.write_str("LocalNode { name: ")?;
                std::fmt::Debug::fmt(&child.name, f)?;
                f.write_str(", game_object: ")?;
                std::fmt::Debug::fmt(&child.game_object, f)?;
                f.write_str(", transform: ")?;
                std::fmt::Debug::fmt(&child.transform, f)?;
                f.write_str(", children: [")?;
                stack.push((child, 0));
                opened.push(0);
            } else {
                f.write_str("] }")?;
                stack.pop();
                opened.pop();
            }
        }
        Ok(())
    }
}

impl Clone for LocalNode {
    fn clone(&self) -> Self {
        // 所有木の複写を再帰にしない。幅優先で通し番号を振り、
        // 深い方から組み立てて親の子列へ移す。子順序は保つ。
        let mut order: Vec<&LocalNode> = vec![self];
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut cursor = 0;
        while cursor < order.len() {
            let children: Vec<&LocalNode> = order[cursor].children.iter().collect();
            let start = order.len();
            order.extend(children);
            ranges.push((start, order.len()));
            cursor += 1;
        }
        let mut built: Vec<Option<LocalNode>> = order
            .iter()
            .map(|node| {
                Some(LocalNode {
                    name: node.name.clone(),
                    game_object: node.game_object,
                    transform: node.transform,
                    children: Vec::with_capacity(node.children.len()),
                })
            })
            .collect();
        for position in (0..order.len()).rev() {
            // 子は通し番号が親より大きいため、深い方から組み立て済みである。
            let (start, end) = ranges[position];
            let mut children = Vec::with_capacity(end - start);
            for slot in &mut built[start..end] {
                children.push(slot.take().expect("子は先に組み立てる"));
            }
            built[position].as_mut().expect("未確定である").children = children;
        }
        built[0].take().expect("rootは残る")
    }
}

impl Drop for LocalNode {
    fn drop(&mut self) {
        // 所有木の破棄を再帰にしない。深い鎖でも呼び出し側のstackを使い果たさない。
        let mut stack = std::mem::take(&mut self.children);
        while let Some(mut node) = stack.pop() {
            let children = std::mem::take(&mut node.children);
            // 子を空にした `node` の破棄は再帰しない。
            drop(node);
            stack.extend(children);
        }
    }
}

/// 階層の構造参照がローカル一致として使えることを確かめる。
///
/// `fileID != 0` かつ `guid` がある参照は外部構成であり、単一ファイルの
/// 階層には組み込めない。ローカル階層を構成できないため合成の非対応として
/// 失敗する。null（`fileID == 0`）はGUIDの有無に優先してnullとして扱い、
/// ここでは通す。
fn ensure_local(reference: &ObjectRef, file_id: FileId) -> Result<(), UnityError> {
    if !reference.is_null() && reference.guid.is_some() {
        return Err(UnityError::new(
            UnityErrorKind::UnsupportedPrefabComposition,
            "外部GUIDへの参照を含むため階層を組み立てない",
        )
        .with_file_id(file_id));
    }
    Ok(())
}

/// 階層を組み立てる。
pub(crate) fn build_hierarchy(
    file: &SerializedFile,
    options: &HierarchyOptions,
) -> Result<LocalHierarchy, UnityError> {
    // 部分的な合成はしない。汎用読み取り自体は可能なままにする。
    if file.documents().iter().any(|document| {
        document.stripped()
            || document.class_id() == 1001
            || document.class_name() == "PrefabInstance"
    }) {
        return Err(UnityError::new(
            UnityErrorKind::UnsupportedPrefabComposition,
            "strippedやPrefabInstanceを含むため階層を組み立てない",
        ));
    }
    // 型付き読み取りの失敗はそのまま返す。未知クラスはここでは無視しない。
    let mut game_objects: HashMap<FileId, (String, Vec<ObjectRef>)> = HashMap::new();
    let mut transforms: HashMap<FileId, Transform> = HashMap::new();
    let mut transform_order: Vec<FileId> = Vec::new();
    for document in file.documents() {
        match document.decode()? {
            TypedObject::GameObject(game_object) => {
                game_objects.insert(
                    document.file_id(),
                    (game_object.name, game_object.components),
                );
            }
            TypedObject::Transform(transform) => {
                transform_order.push(document.file_id());
                transforms.insert(document.file_id(), transform);
            }
            _ => {}
        }
    }
    // Transformの所属先は存在するGameObjectでなければならない。
    // 外部GUIDへの参照はローカル一致として扱わない。
    for (file_id, transform) in &transforms {
        if transform.game_object.is_null() {
            return Err(UnityError::new(
                UnityErrorKind::InvalidHierarchy,
                "Transformの所属先ゲームオブジェクトがnullである",
            )
            .with_file_id(*file_id));
        }
        ensure_local(&transform.game_object, *file_id)?;
        if !game_objects.contains_key(&transform.game_object.file_id) {
            return Err(UnityError::new(
                UnityErrorKind::InvalidHierarchy,
                "Transformの所属先ゲームオブジェクトがない",
            )
            .with_file_id(*file_id));
        }
    }
    // 各GameObjectはちょうど1つのTransformに属さなければならない。
    // RectTransformなどの未知型だけを持つ構成は対象外として拒否する。
    let mut owner_of: HashMap<FileId, FileId> = HashMap::new();
    for (file_id, transform) in &transforms {
        if let Some(previous) = owner_of.insert(transform.game_object.file_id, *file_id) {
            return Err(UnityError::new(
                UnityErrorKind::InvalidHierarchy,
                format!("ゲームオブジェクトが複数のTransformに属する: {previous}と{file_id}"),
            )
            .with_file_id(transform.game_object.file_id));
        }
    }
    for game_id in game_objects.keys() {
        if !owner_of.contains_key(game_id) {
            return Err(UnityError::new(
                UnityErrorKind::InvalidHierarchy,
                "Transformを持たないゲームオブジェクトがある",
            )
            .with_file_id(*game_id));
        }
    }
    // 構成要素の参照先は存在する文書でなければならない。
    // GameObjectとTransformの双方向対応も検証する。対応するTransformを
    // 含まない構成要素や、他のGameObjectのTransform・GameObject自体への
    // 取り違えは階層として成立しない。
    for (game_id, (_, components)) in &game_objects {
        for reference in components {
            if reference.is_null() {
                continue;
            }
            ensure_local(reference, *game_id)?;
            if file.document(reference.file_id).is_none() {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "構成要素の参照先文書がない",
                )
                .with_file_id(*game_id));
            }
        }
        let owner = owner_of.get(game_id).expect("所有の存在は確かめた");
        if !components.iter().any(|reference| {
            !reference.is_null() && reference.guid.is_none() && reference.file_id == *owner
        }) {
            return Err(UnityError::new(
                UnityErrorKind::InvalidHierarchy,
                "GameObjectの構成要素に対応するTransformがない",
            )
            .with_file_id(*game_id));
        }
        for reference in components {
            if reference.is_null() {
                continue;
            }
            // 上の検査と重複するが、順序を変えても外部参照を黙って通さない。
            ensure_local(reference, *game_id)?;
            if game_objects.contains_key(&reference.file_id) {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "構成要素がゲームオブジェクトを指す",
                )
                .with_file_id(*game_id));
            }
            if let Some(other) = transforms.get(&reference.file_id)
                && other.game_object.file_id != *game_id
            {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "構成要素が他のゲームオブジェクトのTransformを指す",
                )
                .with_file_id(*game_id));
            }
        }
    }
    // 親子の整合性を検証する。子の順序は `m_Children` のまま保つ。
    for (file_id, transform) in &transforms {
        if !transform.father.is_null() {
            ensure_local(&transform.father, *file_id)?;
            let father = transforms.get(&transform.father.file_id).ok_or_else(|| {
                UnityError::new(UnityErrorKind::InvalidHierarchy, "親Transformの文書がない")
                    .with_file_id(*file_id)
            })?;
            if !father
                .children
                .iter()
                .any(|child| child.file_id == *file_id)
            {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "親子が不整合である: 親の子一覧に自分がない",
                )
                .with_file_id(*file_id));
            }
        }
        let mut seen_children: HashSet<FileId> = HashSet::new();
        for child in &transform.children {
            if child.is_null() {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "子Transformの参照がnullである",
                )
                .with_file_id(*file_id));
            }
            ensure_local(child, *file_id)?;
            if !seen_children.insert(child.file_id) {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "子Transformが重複する",
                )
                .with_file_id(*file_id));
            }
            let child_transform = transforms.get(&child.file_id).ok_or_else(|| {
                UnityError::new(UnityErrorKind::InvalidHierarchy, "子Transformの文書がない")
                    .with_file_id(*file_id)
            })?;
            if child_transform.father.file_id != *file_id {
                return Err(UnityError::new(
                    UnityErrorKind::InvalidHierarchy,
                    "親子が不整合である: 子の親が自分でない",
                )
                .with_file_id(*file_id));
            }
        }
    }
    // rootから辿って深さを求め、反復処理で組み立てる。再帰は使わない。
    // 親子対応は上で両方向を確かめたため、rootから辿れる構造は森になる。
    // 循環は辿り残しとして検出し、深さ超過は資源上限として失敗する。
    let mut depth_of: HashMap<FileId, usize> = HashMap::new();
    let mut exit_order: Vec<FileId> = Vec::new();
    {
        enum Frame {
            Enter(FileId, usize),
            Exit(FileId),
        }
        let mut visited: HashSet<FileId> = HashSet::new();
        for file_id in &transform_order {
            let transform = transforms.get(file_id).expect("索引済みである");
            if !transform.father.is_null() {
                continue;
            }
            if !visited.insert(*file_id) {
                continue;
            }
            let mut stack = vec![Frame::Enter(*file_id, 0)];
            while let Some(frame) = stack.pop() {
                match frame {
                    Frame::Enter(current, depth) => {
                        if depth > options.max_depth {
                            return Err(UnityError::new(
                                UnityErrorKind::LimitExceeded,
                                format!("階層の深さが上限を超えた: {depth}"),
                            )
                            .with_file_id(current));
                        }
                        depth_of.insert(current, depth);
                        stack.push(Frame::Exit(current));
                        let current_transform = transforms.get(&current).expect("索引済みである");
                        for child in current_transform.children.iter().rev() {
                            if !visited.insert(child.file_id) {
                                return Err(UnityError::new(
                                    UnityErrorKind::InvalidHierarchy,
                                    "Transformの循環がある",
                                )
                                .with_file_id(child.file_id));
                            }
                            stack.push(Frame::Enter(child.file_id, depth + 1));
                        }
                    }
                    Frame::Exit(current) => exit_order.push(current),
                }
            }
        }
    }
    if depth_of.len() != transforms.len() {
        return Err(UnityError::new(
            UnityErrorKind::InvalidHierarchy,
            "rootから辿れないTransformがある",
        ));
    }
    // 深い方から組み立てる。子は親より先に確定するため、再帰は要らない。
    let mut built: HashMap<FileId, LocalNode> = HashMap::new();
    for file_id in exit_order {
        let transform = transforms.get(&file_id).expect("索引済みである");
        let (name, _) = game_objects
            .get(&transform.game_object.file_id)
            .expect("所属先の存在は確かめた");
        let mut children = Vec::with_capacity(transform.children.len());
        for child in &transform.children {
            children.push(built.remove(&child.file_id).expect("子は先に組み立てる"));
        }
        built.insert(
            file_id,
            LocalNode {
                name: name.clone(),
                game_object: transform.game_object.file_id,
                transform: file_id,
                children,
            },
        );
    }
    let mut roots = Vec::new();
    for file_id in &transform_order {
        let transform = transforms.get(file_id).expect("索引済みである");
        if transform.father.is_null() {
            roots.push(built.remove(file_id).expect("rootは組み立てた"));
        }
    }
    Ok(LocalHierarchy { roots })
}
