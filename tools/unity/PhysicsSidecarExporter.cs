// vrctom PhysicsSidecarExporter
// VRChat SDK 3.10.5 の PhysBone / PhysBoneCollider を読み取り、
// Rust 側のスプリングボーン変換が使う物理 sidecar JSON (format: vrctom.physics, schemaVersion: 1) を書き出すエディタヘルパ。
// シーンや Prefab を保存・変更しない。Edit Mode の値をそのまま読む。
using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;
using VRC.Dynamics;
using VRC.SDK3.Avatars.Components;
using VRC.SDK3.Dynamics.PhysBone.Components;

/// <summary>
/// 物理 sidecar JSON を書き出す。GUI からは Tools/vrctom/Export Physics Sidecar、
/// バッチ実行では -executeMethod PhysicsSidecarExporter.Run で呼び出す。
/// </summary>
public static class PhysicsSidecarExporter
{
    // 既定の出力先。環境変数 VRCTOM_PHYSICS_OUT があればそちらを優先する。
    private const string DefaultOutPath =
        "/private/var/folders/3_/9fhdxzs960qgg34dctd97b840000gn/T/opencode/unity-export/milfy-physics.json";

    // 開いているシーンにアバターが見当たらない場合に開く検証用シーン。
    private const string FallbackScenePath = "Assets/Scenes/MilfyCheck.unity";

    private const string FormatName = "vrctom.physics";
    private const int SchemaVersion = 1;

    /// <summary>GUI メニューからの実行口。環境変数 VRCTOM_PHYSICS_STATE に従う (既定 bind-pose)。</summary>
    [MenuItem("Tools/vrctom/Export Physics Sidecar")]
    public static void ExportFromMenu()
    {
        ExportWithPolicy(ReadStatePolicy(), failQuit: false);
    }

    /// <summary>GUI メニューからの bind-pose 実行口。エディタは終了しない。</summary>
    [MenuItem("Tools/vrctom/Export Physics Sidecar (Bind Pose)")]
    public static void ExportBindPoseFromMenu()
    {
        ExportWithPolicy(CaptureStatePolicy.BindPose, failQuit: false);
    }

    /// <summary>GUI メニューからの edit-mode 実行口。エディタは終了しない。</summary>
    [MenuItem("Tools/vrctom/Export Physics Sidecar (Edit Mode)")]
    public static void ExportEditModeFromMenu()
    {
        ExportWithPolicy(CaptureStatePolicy.EditMode, failQuit: false);
    }

    /// <summary>-executeMethod 用の実行口。環境変数に従い、成功で 0、失敗で 1 を返して終了する。</summary>
    public static void Run()
    {
        ExportWithPolicy(ReadStatePolicy(), failQuit: true);
    }

    /// <summary>-executeMethod 用の bind-pose 実行口。成功で 0、失敗で 1 を返して終了する。</summary>
    public static void RunBindPose()
    {
        ExportWithPolicy(CaptureStatePolicy.BindPose, failQuit: true);
    }

    /// <summary>-executeMethod 用の edit-mode 実行口。成功で 0、失敗で 1 を返して終了する。</summary>
    public static void RunEditMode()
    {
        ExportWithPolicy(CaptureStatePolicy.EditMode, failQuit: true);
    }

    // 参照姿勢の方針。bind-pose が既定で、環境変数 VRCTOM_PHYSICS_STATE で切替える。
    private enum CaptureStatePolicy
    {
        BindPose,
        EditMode,
    }

    private static CaptureStatePolicy ReadStatePolicy()
    {
        string raw = Environment.GetEnvironmentVariable("VRCTOM_PHYSICS_STATE");
        if (string.IsNullOrEmpty(raw)) return CaptureStatePolicy.BindPose;
        if (raw.Equals("bind-pose", StringComparison.OrdinalIgnoreCase)) return CaptureStatePolicy.BindPose;
        if (raw.Equals("edit-mode", StringComparison.OrdinalIgnoreCase)) return CaptureStatePolicy.EditMode;
        throw new InvalidOperationException(
            "VRCTOM_PHYSICS_STATE が不正 (" + raw + ")。bind-pose か edit-mode を指定すること。");
    }

    private static string StatePolicyName(CaptureStatePolicy policy)
    {
        return policy == CaptureStatePolicy.BindPose ? "bind-pose" : "edit-mode";
    }

    private static void ExportWithPolicy(CaptureStatePolicy policy, bool failQuit)
    {
        string outPath = Environment.GetEnvironmentVariable("VRCTOM_PHYSICS_OUT");
        if (string.IsNullOrEmpty(outPath)) outPath = DefaultOutPath;

        try
        {
            GameObject avatarRoot = ResolveAvatarRoot(allowSceneFallback: Application.isBatchMode);
            var stats = new ReferenceStats();
            SidecarDto sidecar = BuildSidecar(avatarRoot, policy, stats);
            ValidateSidecar(sidecar);
            // JsonUtility は null 文字列を "" に、null の入れ子オブジェクトを
            // 既定値のオブジェクトに展開するため、スキーマの null 表現に直す。
            // 対象はこの DTO 固有の形だけに絞る (parentId の空文字、source の既定形)。
            string json = JsonUtility.ToJson(sidecar, true);
            json = NullParentPattern.Replace(json, "\"parentId\": null");
            json = NullSourcePattern.Replace(json, "\"source\": null");
            Directory.CreateDirectory(Path.GetDirectoryName(outPath));
            // BOM 無し UTF-8 で書き出す。Rust 側の読み取りと差分比較を安定させる。
            File.WriteAllText(outPath, json, new UTF8Encoding(false));
            Debug.Log(string.Format(
                "[PhysicsSidecarExporter] wrote {0} policy={1} (nodes={2} bind={3} scene={4} approx={5} changed={6} maxAngle={7:F4}deg maxPos={8:E2} maxScale={9:E2} physBones={10} colliders={11} unsupported={12})",
                outPath, StatePolicyName(policy), sidecar.nodes.Length,
                stats.BindNodes, stats.SceneFallbacks, stats.ApproxDecomposed, stats.ChangedFromScene,
                stats.MaxAngleDeg, stats.MaxPosDiff, stats.MaxScaleDiff,
                sidecar.physBones.Length, sidecar.colliders.Length,
                sidecar.unsupportedFeatures.Length));
            if (stats.ChangedIds.Count > 0)
            {
                Debug.Log("[PhysicsSidecarExporter] bind changed: "
                    + string.Join(", ", stats.ChangedIds.ToArray())
                    + (stats.ChangedFromScene > stats.ChangedIds.Count
                        ? string.Format(" (+{0} more)", stats.ChangedFromScene - stats.ChangedIds.Count)
                        : string.Empty));
            }
            if (Application.isBatchMode) EditorApplication.Exit(0);
        }
        catch (Exception e)
        {
            Debug.LogError("[PhysicsSidecarExporter] " + e.Message + "\n" + e.StackTrace);
            if (Application.isBatchMode || failQuit) EditorApplication.Exit(1);
        }
    }

    // アバタールート (VRCAvatarDescriptor の GameObject) を探す。
    // 選択中がアバターならそれを優先し、なければシーン内を1件だけ探す。複数はエラー。
    private static GameObject ResolveAvatarRoot(bool allowSceneFallback)
    {
        GameObject selected = Selection.activeGameObject;
        if (selected != null && selected.GetComponent<VRCAvatarDescriptor>() != null) return selected;

        VRCAvatarDescriptor found = FindSingleDescriptor();
        if (found == null && allowSceneFallback) found = OpenFallbackSceneAndFind();
        if (found == null)
            throw new InvalidOperationException(
                "シーン内に VRCAvatarDescriptor が見つからない。アバターを選択するか MilfyCheck.unity を開いて実行すること。");
        return found.gameObject;
    }

    private static VRCAvatarDescriptor FindSingleDescriptor()
    {
        VRCAvatarDescriptor[] all = UnityEngine.Object.FindObjectsOfType<VRCAvatarDescriptor>(true);
        if (all.Length > 1)
            throw new InvalidOperationException(
                "シーン内に VRCAvatarDescriptor が複数ある。書き出すアバターを選択してから実行すること。");
        return all.Length == 1 ? all[0] : null;
    }

    private static VRCAvatarDescriptor OpenFallbackSceneAndFind()
    {
        string full = Path.Combine(Application.dataPath, FallbackScenePath.Substring("Assets/".Length));
        if (!File.Exists(full)) return null;
        Scene active = SceneManager.GetActiveScene();
        if (active.path != FallbackScenePath)
        {
            // 読み取り専用で開くだけ。保存はしない。
            EditorSceneManager.OpenScene(FallbackScenePath);
        }
        return FindSingleDescriptor();
    }

    private static SidecarDto BuildSidecar(GameObject avatarRoot, CaptureStatePolicy policy, ReferenceStats stats)
    {
        // ノードはアバタールート直下から DFS (自身は含めない)。n:0 は最初の子孫になる。
        var order = new List<Transform>();
        CollectDescendants(avatarRoot.transform, order);
        if (order.Count == 0)
            throw new InvalidOperationException("アバター配下に Transform が無い。");

        var nodeIndex = new Dictionary<Transform, int>(order.Count);
        for (int i = 0; i < order.Count; i++) nodeIndex[order[i]] = i;

        // bind-pose では Body の bind 行列から local を復元する。edit-mode では使わない。
        ReferenceFrame frame = ReferenceFrame.Select(avatarRoot, policy);

        var nodes = new NodeDto[order.Count];
        var fbxSources = new SortedDictionary<string, SourceModelDto>(StringComparer.Ordinal);
        var unsupported = new HashSet<string>(StringComparer.Ordinal);
        for (int i = 0; i < order.Count; i++)
            nodes[i] = BuildNode(avatarRoot, order[i], nodeIndex, fbxSources, frame, stats, unsupported);

        // コライダーを先に ID 化する。PhysBone 側の参照解決に使う。
        // 収録対象は Edit Mode で有効なものだけ (component.enabled かつ activeInHierarchy)。
        // 無効な SmartPhone 系を除くと Milfy では PhysBone 39 件・Collider 12 件になる。
        VRCPhysBoneCollider[] colliderComponents = SortedComponents<VRCPhysBoneCollider>(avatarRoot, nodeIndex);
        var enabledColliders = new List<VRCPhysBoneCollider>(colliderComponents.Length);
        foreach (VRCPhysBoneCollider collider in colliderComponents)
        {
            if (collider.enabled && collider.gameObject.activeInHierarchy) enabledColliders.Add(collider);
        }
        var colliderIdOf = new Dictionary<Component, string>();
        var colliders = new ColliderDto[enabledColliders.Count];
        for (int i = 0; i < enabledColliders.Count; i++)
        {
            string id = "c:" + i;
            colliderIdOf[enabledColliders[i]] = id;
            colliders[i] = BuildCollider(enabledColliders[i], id, nodeIndex);
            if (enabledColliders[i].shapeType == VRCPhysBoneColliderBase.ShapeType.Plane)
                unsupported.Add("plane-colliders");
        }

        VRCPhysBone[] physBoneComponents = SortedComponents<VRCPhysBone>(avatarRoot, nodeIndex);
        var enabledBones = new List<VRCPhysBone>(physBoneComponents.Length);
        foreach (VRCPhysBone bone in physBoneComponents)
        {
            if (bone.enabled && bone.gameObject.activeInHierarchy) enabledBones.Add(bone);
        }
        var physBones = new PhysBoneDto[enabledBones.Count];
        for (int i = 0; i < enabledBones.Count; i++)
            physBones[i] = BuildPhysBone(enabledBones[i], "pb:" + i, nodeIndex, colliderIdOf, unsupported);

        var unsupportedSorted = new List<string>(unsupported);
        unsupportedSorted.Sort(StringComparer.Ordinal);

        Transform first = order[0];
        return new SidecarDto
        {
            format = FormatName,
            schemaVersion = SchemaVersion,
            generator = new GeneratorDto
            {
                name = "vrctom PhysicsSidecarExporter",
                version = "1",
                unityVersion = Application.unityVersion,
                sdkVersion = ReadSdkVersion(),
            },
            capture = new CaptureDto
            {
                statePolicy = StatePolicyName(policy),
                coordinateSpace = "unity-avatar-root-local-meters",
                avatarRootId = "n:0",
                rootScale = Float3(first.localScale),
                bindPoseSource = frame.BindPoseSourcePath ?? new string[0],
            },
            sourceModels = new List<SourceModelDto>(fbxSources.Values).ToArray(),
            nodes = nodes,
            physBones = physBones,
            colliders = colliders,
            unsupportedFeatures = unsupportedSorted.ToArray(),
        };
    }

    private static void CollectDescendants(Transform parent, List<Transform> into)
    {
        for (int i = 0; i < parent.childCount; i++)
        {
            Transform child = parent.GetChild(i);
            into.Add(child);
            CollectDescendants(child, into);
        }
    }

    // GetComponentsInChildren は階層順で返るが、念のためノード DFS 順→取得順で並べ直して決定性を保つ。
    private static T[] SortedComponents<T>(GameObject avatarRoot, Dictionary<Transform, int> nodeIndex)
        where T : Component
    {
        T[] found = avatarRoot.GetComponentsInChildren<T>(true);
        var original = new Dictionary<T, int>(found.Length);
        for (int i = 0; i < found.Length; i++) original[found[i]] = i;
        Array.Sort(found, (a, b) =>
        {
            int order = nodeIndex[a.transform].CompareTo(nodeIndex[b.transform]);
            return order != 0 ? order : original[a].CompareTo(original[b]);
        });
        return found;
    }

    private static NodeDto BuildNode(
        GameObject avatarRoot, Transform t, Dictionary<Transform, int> nodeIndex,
        SortedDictionary<string, SourceModelDto> fbxSources, ReferenceFrame frame, ReferenceStats stats,
        HashSet<string> unsupported)
    {
        Vector3 position;
        Quaternion rotation;
        Vector3 scale;
        string referenceSource;
        Matrix4x4 bindWorld;
        if (frame.BindWorlds != null && frame.BindWorlds.TryGetValue(t, out bindWorld))
        {
            // 親が同じ SMR のボーンなら bind 世界行列、そうでなければ scene の現在行列を使う。
            Transform parent = t.parent;
            Matrix4x4 parentBindWorld;
            Matrix4x4 parentWorld = parent != null && frame.BindWorlds.TryGetValue(parent, out parentBindWorld)
                ? parentBindWorld
                : (parent != null ? parent.localToWorldMatrix : Matrix4x4.identity);
            if (DecomposeBindLocal(parentWorld.inverse * bindWorld, out position, out rotation, out scale))
            {
                referenceSource = "bind";
            }
            else
            {
                // せん断等で TRS に戻らない行列。黙って近似せず申告する。
                referenceSource = "bind-decomposed-approx";
                unsupported.Add("trs-decomposition:n:" + nodeIndex[t]);
                stats.ApproxDecomposed++;
            }
            stats.BindNodes++;
            float angleDeg = Quaternion.Angle(
                Quaternion.Normalize(rotation), Quaternion.Normalize(t.localRotation));
            float posDiff = (position - t.localPosition).magnitude;
            float scaleDiff = (scale - t.localScale).magnitude;
            if (angleDeg > stats.MaxAngleDeg) stats.MaxAngleDeg = angleDeg;
            if (posDiff > stats.MaxPosDiff) stats.MaxPosDiff = posDiff;
            if (scaleDiff > stats.MaxScaleDiff) stats.MaxScaleDiff = scaleDiff;
            // 値としての変化だけを数える (FBX 由来の非正規化は除く)。判定には使わない。
            if (angleDeg > 1e-3f || posDiff > 1e-6f || scaleDiff > 1e-6f)
            {
                stats.ChangedFromScene++;
                if (stats.ChangedIds.Count < 40) stats.ChangedIds.Add("n:" + nodeIndex[t] + " " + t.name);
            }
        }
        else
        {
            // bindposes に無いノード (Ground 等) は scene の local をそのまま使う。負スケールも保持する。
            position = t.localPosition;
            rotation = t.localRotation;
            scale = t.localScale;
            referenceSource = "scene";
            stats.SceneFallbacks++;
        }
        return new NodeDto
        {
            id = "n:" + nodeIndex[t],
            parentId = t.parent == avatarRoot.transform ? null : "n:" + nodeIndex[t.parent],
            name = t.name,
            siblingIndex = t.GetSiblingIndex(),
            scenePath = BuildScenePath(avatarRoot.transform, t),
            source = BuildSource(t, fbxSources),
            referenceSource = referenceSource,
            referenceLocal = new TransformDto
            {
                position = Float3(position),
                rotation = Float4(rotation),
                scale = Float3(scale),
            },
        };
    }

    // bind 参照の解決器。edit-mode では BindWorlds を作らず、全ノードが scene 扱いになる。
    private sealed class ReferenceFrame
    {
        public Dictionary<Transform, Matrix4x4> BindWorlds;
        public string[] BindPoseSourcePath;

        public static ReferenceFrame Select(GameObject avatarRoot, CaptureStatePolicy policy)
        {
            var frame = new ReferenceFrame();
            if (policy != CaptureStatePolicy.BindPose) return frame;
            SkinnedMeshRenderer smr = SelectBindSourceMesh(avatarRoot);
            Transform[] bones = smr.bones;
            Matrix4x4[] bindposes = smr.sharedMesh.bindposes;
            if (bones == null || bindposes == null || bones.Length != bindposes.Length)
            {
                throw new InvalidOperationException(
                    "bindposes と bones の数が合わない (" + smr.name + ")。メッシュが壊れている。");
            }
            // bindpose はボーン bind 時の世界行列の逆行列 (メッシュ基準) のため、
            // メッシュの現在行列を掛けて bind 時のボーン世界行列に戻す。
            // メッシュ自身の Transform は bind から動かさない前提 (Body 等)。
            Matrix4x4 meshWorld = smr.transform.localToWorldMatrix;
            var worlds = new Dictionary<Transform, Matrix4x4>(bones.Length);
            for (int i = 0; i < bones.Length; i++)
            {
                if (bones[i] == null) continue;
                worlds[bones[i]] = meshWorld * bindposes[i].inverse;
            }
            frame.BindWorlds = worlds;
            frame.BindPoseSourcePath = BuildScenePath(avatarRoot.transform, smr.transform);
            return frame;
        }

        // アバター配下でボーン数が最も多い SkinnedMeshRenderer を選ぶ (Milfy では Body)。
        // ボーン数が同点の場合は Body という名前 (VRChat の代表メッシュの慣例) を優先し、
        // 次に頂点数が最も多いものを選ぶ。それも同点なら階層順で最初を優先して決定性を保つ。
        private static SkinnedMeshRenderer SelectBindSourceMesh(GameObject avatarRoot)
        {
            SkinnedMeshRenderer best = null;
            int bestBones = -1;
            bool bestIsBody = false;
            int bestVertices = -1;
            foreach (SkinnedMeshRenderer smr in avatarRoot.GetComponentsInChildren<SkinnedMeshRenderer>(true))
            {
                if (smr.sharedMesh == null || smr.bones == null) continue;
                int bones = smr.bones.Length;
                bool isBody = smr.name.Equals("Body", StringComparison.OrdinalIgnoreCase);
                int vertices = smr.sharedMesh.vertexCount;
                Debug.Log(string.Format(
                    "[PhysicsSidecarExporter] bind 候補 {0} bones={1} bindposes={2} vertices={3} active={4}",
                    string.Join("/", BuildScenePath(avatarRoot.transform, smr.transform)),
                    bones, smr.sharedMesh.bindposes.Length, vertices,
                    smr.gameObject.activeInHierarchy));
                if (bones > bestBones
                    || (bones == bestBones && isBody && !bestIsBody)
                    || (bones == bestBones && isBody == bestIsBody && vertices > bestVertices))
                {
                    best = smr;
                    bestBones = bones;
                    bestIsBody = isBody;
                    bestVertices = vertices;
                }
            }
            if (best == null)
                throw new InvalidOperationException("bind-pose 用の SkinnedMeshRenderer がアバター配下に無い。");
            Debug.Log("[PhysicsSidecarExporter] bind 採用 " + best.name + " bones=" + bestBones);
            return best;
        }
    }

    // ノード集計。bind 適用数、scene フォールバック数、scene と値が変わった数を数える。
    private sealed class ReferenceStats
    {
        public int BindNodes;
        public int SceneFallbacks;
        public int ApproxDecomposed;
        public int ChangedFromScene;
        public float MaxAngleDeg;
        public float MaxPosDiff;
        public float MaxScaleDiff;
        public readonly List<string> ChangedIds = new List<string>();
    }

    // bind 世界行列の差分からローカル TRS を取り出す。鏡像 (負スケール) を保つ。
    // det3 < 0 のとき X 軸の scale を負にし、回転部を固有回転に戻す。
    // TRS で元行列に戻るか検証し、戻らない (せん断等) 場合は false を返す。
    private static bool DecomposeBindLocal(
        Matrix4x4 local, out Vector3 position, out Quaternion rotation, out Vector3 scale)
    {
        position = local.GetColumn(3);
        Vector3 x = local.GetColumn(0);
        Vector3 y = local.GetColumn(1);
        Vector3 z = local.GetColumn(2);
        const float eps = 1e-8f;
        float sx = x.magnitude;
        float sy = y.magnitude;
        float sz = z.magnitude;
        Vector3 nx = sx > eps ? x / sx : Vector3.right;
        Vector3 ny = sy > eps ? y / sy : Vector3.up;
        Vector3 nz = sz > eps ? z / sz : Vector3.forward;
        if (Vector3.Dot(Vector3.Cross(x, y), z) < 0f)
        {
            // 鏡像は X 軸に寄せる。回転部は固有回転に戻る。
            nx = -nx;
            sx = -sx;
        }
        scale = new Vector3(sx, sy, sz);
        rotation = new Matrix4x4(nx, ny, nz, new Vector4(0, 0, 0, 1)).rotation;
        Matrix4x4 rebuilt = Matrix4x4.TRS(position, rotation, scale);
        for (int i = 0; i < 16; i++)
        {
            if (Mathf.Abs(rebuilt[i] - local[i]) > 1e-4f) return false;
        }
        return true;
    }

    private static string[] BuildScenePath(Transform avatarRoot, Transform node)
    {
        var names = new List<string>();
        for (Transform t = node; t != null && t != avatarRoot; t = t.parent) names.Add(t.name);
        names.Add(avatarRoot.name);
        names.Reverse();
        return names.ToArray();
    }

    private static SourceRefDto BuildSource(Transform t, SortedDictionary<string, SourceModelDto> fbxSources)
    {
        // シーン→Prefab→FBX と対応元を遡り、最も外側の FBX 由来オブジェクトを使う。
        // Prefab 直付けやシーン追加のノードは対応元が FBX に届かないため source: null になる。
        Transform src = t;
        while (true)
        {
            var outer = PrefabUtility.GetCorrespondingObjectFromSource(src) as Transform;
            if (outer == null) break;
            src = outer;
        }
        if (src == t) return null;

        string assetPath = AssetDatabase.GetAssetPath(src);
        if (string.IsNullOrEmpty(assetPath) || !assetPath.EndsWith(".fbx", StringComparison.OrdinalIgnoreCase))
            return null;

        // FBX ルート名は含めず、その配下の名前列だけを path にする。
        // src から同じアセットの親を辿り、最も外側 (FBX ルート) の手前までを集める。
        var names = new List<string>();
        for (Transform s = src;
            s != null && s.parent != null && AssetDatabase.GetAssetPath(s.parent) == assetPath;
            s = s.parent)
            names.Add(s.name);
        names.Reverse();
        // src 自身が FBX ルート (モデル直置き) の場合は名前が残らないため、
        // 自身名を path とする。空 path を作らないための例外規定。
        if (names.Count == 0) names.Add(src.name);

        string guid;
        long localId;
        if (!AssetDatabase.TryGetGUIDAndLocalFileIdentifier(src, out guid, out localId))
            throw new InvalidOperationException("FBX 由来ノードの GUID 取得に失敗した:" + assetPath);

        string modelId = Path.GetFileNameWithoutExtension(assetPath).ToLowerInvariant();
        if (!fbxSources.ContainsKey(assetPath))
        {
            fbxSources[assetPath] = new SourceModelDto
            {
                id = modelId,
                sha256 = Sha256OfAsset(assetPath),
                assetGuid = AssetDatabase.AssetPathToGUID(assetPath),
                assetPath = assetPath,
            };
        }

        return new SourceRefDto
        {
            modelId = modelId,
            path = names.ToArray(),
            assetGuid = guid,
            localId = localId,
        };
    }

    private static string Sha256OfAsset(string assetPath)
    {
        string full = Path.Combine(Path.GetDirectoryName(Application.dataPath), assetPath);
        using (SHA256 sha = SHA256.Create())
        using (FileStream stream = File.OpenRead(full))
        {
            byte[] hash = sha.ComputeHash(stream);
            var sb = new StringBuilder(64);
            foreach (byte b in hash) sb.Append(b.ToString("x2"));
            return sb.ToString();
        }
    }

    private static PhysBoneDto BuildPhysBone(
        VRCPhysBone bone, string id, Dictionary<Transform, int> nodeIndex,
        Dictionary<Component, string> colliderIdOf, HashSet<string> unsupported)
    {
        var dto = new PhysBoneDto
        {
            id = id,
            componentNodeId = "n:" + nodeIndex[bone.transform],
            rootNodeId = ResolveRootNodeId(bone.transform, bone.rootTransform, nodeIndex, id, unsupported),
            enabled = bone.enabled && bone.gameObject.activeInHierarchy,
            version = PhysBoneVersionName(bone.version),
            integrationType = bone.integrationType.ToString(),
            ignoreOtherPhysBones = bone.ignoreOtherPhysBones,
            endpointPosition = Float3(bone.endpointPosition),
            multiChildType = bone.multiChildType.ToString(),
            pull = Curved(bone.pull, bone.pullCurve),
            spring = Curved(bone.spring, bone.springCurve),
            stiffness = Curved(bone.stiffness, bone.stiffnessCurve),
            gravity = Curved(bone.gravity, bone.gravityCurve),
            gravityFalloff = Curved(bone.gravityFalloff, bone.gravityFalloffCurve),
            immobileType = bone.immobileType.ToString(),
            immobile = Curved(bone.immobile, bone.immobileCurve),
            radius = Curved(bone.radius, bone.radiusCurve),
            // SDK 3.10.5 の allowCollision は AdvancedBool enum のため、
            // 衝突が有効な True/Other を true に丸める。詳細は README の制限節を参照。
            allowCollision = bone.allowCollision != VRCPhysBoneBase.AdvancedBool.False,
            limitType = bone.limitType.ToString(),
            maxAngleX = Curved(bone.maxAngleX, bone.maxAngleXCurve),
            maxAngleZ = Curved(bone.maxAngleZ, bone.maxAngleZCurve),
            limitRotation = Float3(bone.limitRotation),
            // 制限回転の軸別カーブ。対応する float 値が無いため value は既定の 1。
            limitRotationXCurve = LimitCurve(bone.limitRotationXCurve),
            limitRotationYCurve = LimitCurve(bone.limitRotationYCurve),
            limitRotationZCurve = LimitCurve(bone.limitRotationZCurve),
            isAnimated = bone.isAnimated,
            ignoreNodeIds = new string[0],
            colliderIds = new string[0],
        };

        if (bone.ignoreTransforms != null)
        {
            var ignore = new List<string>();
            foreach (Transform ignored in bone.ignoreTransforms)
            {
                int index;
                if (ignored != null && nodeIndex.TryGetValue(ignored, out index)) ignore.Add("n:" + index);
            }
            dto.ignoreNodeIds = ignore.ToArray();
        }

        if (bone.colliders != null)
        {
            var colliderIds = new List<string>();
            bool hasExternal = false;
            foreach (VRCPhysBoneColliderBase collider in bone.colliders)
            {
                if (collider == null) continue;
                string colliderId;
                if (colliderIdOf.TryGetValue(collider, out colliderId)) colliderIds.Add(colliderId);
                else hasExternal = true;
            }
            dto.colliderIds = colliderIds.ToArray();
            if (hasExternal) unsupported.Add("external-colliders:" + id);
        }

        if (bone.stretchMotion != 0f || HasKeys(bone.stretchMotionCurve))
            unsupported.Add("stretch-motion:" + id);
        if (bone.allowGrabbing != VRCPhysBoneBase.AdvancedBool.False)
            unsupported.Add("allow-grabbing:" + id + ":" + bone.allowGrabbing);
        if (bone.allowPosing != VRCPhysBoneBase.AdvancedBool.False)
            unsupported.Add("allow-posing:" + id + ":" + bone.allowPosing);

        return dto;
    }

    private static ColliderDto BuildCollider(
        VRCPhysBoneCollider collider, string id, Dictionary<Transform, int> nodeIndex)
    {
        var dto = new ColliderDto
        {
            id = id,
            componentNodeId = "n:" + nodeIndex[collider.transform],
            rootNodeId = ResolveRootNodeId(collider.transform, collider.rootTransform, nodeIndex, id, null),
            enabled = collider.enabled && collider.gameObject.activeInHierarchy,
            shapeType = collider.shapeType.ToString(),
            insideBounds = collider.insideBounds,
            radius = collider.radius,
            height = collider.height,
            position = Float3(collider.position),
            rotation = Float4(collider.rotation),
            bonesAsSpheres = collider.bonesAsSpheres,
        };
        return dto;
    }

    private static string ResolveRootNodeId(
        Transform componentTransform, Transform rootTransform, Dictionary<Transform, int> nodeIndex,
        string componentId, HashSet<string> unsupported)
    {
        Transform target = rootTransform != null ? rootTransform : componentTransform;
        int index;
        if (nodeIndex.TryGetValue(target, out index)) return "n:" + index;
        // rootTransform がアバター外を指す想定外の入力。落とさず自ノードに読み替えて記録する。
        if (unsupported != null) unsupported.Add("root-transform-outside:" + componentId);
        return "n:" + nodeIndex[componentTransform];
    }

    private static string PhysBoneVersionName(VRCPhysBoneBase.Version version)
    {
        const string prefix = "Version_";
        string name = version.ToString();
        if (name.StartsWith(prefix, StringComparison.Ordinal))
            name = name.Substring(prefix.Length).Replace('_', '.');
        return name;
    }

    private static CurvedDto Curved(float value, AnimationCurve curve)
    {
        return new CurvedDto
        {
            value = value,
            keys = CurveKeys(curve),
            preWrapMode = curve != null ? (int)curve.preWrapMode : 0,
            postWrapMode = curve != null ? (int)curve.postWrapMode : 0,
        };
    }

    // 制限回転の軸別カーブ用。単独の float 値を持たないため value は既定の 1。
    private static CurvedDto LimitCurve(AnimationCurve curve)
    {
        return Curved(1f, curve);
    }

    private static CurveKeyDto[] CurveKeys(AnimationCurve curve)
    {
        if (curve == null || curve.keys == null) return new CurveKeyDto[0];
        Keyframe[] keys = curve.keys;
        var dto = new CurveKeyDto[keys.Length];
        for (int i = 0; i < keys.Length; i++)
        {
            dto[i] = new CurveKeyDto
            {
                time = keys[i].time,
                value = keys[i].value,
                inSlope = keys[i].inTangent,
                outSlope = keys[i].outTangent,
                weightedMode = (int)keys[i].weightedMode,
                inWeight = keys[i].inWeight,
                outWeight = keys[i].outWeight,
            };
        }
        return dto;
    }

    private static bool HasKeys(AnimationCurve curve)
    {
        return curve != null && curve.keys != null && curve.keys.Length > 0;
    }

    private static string ReadSdkVersion()
    {
        try
        {
            string projectDir = Path.GetDirectoryName(Application.dataPath);
            string packageJson = File.ReadAllText(Path.Combine(projectDir, "Packages/com.vrchat.base/package.json"));
            // 最小限の手書き解析。version 値は "x.y.z" 形式の単一行を想定する。
            foreach (string rawLine in packageJson.Split('\n'))
            {
                string line = rawLine.Trim();
                if (!line.StartsWith("\"version\"", StringComparison.Ordinal)) continue;
                int colon = line.IndexOf(':');
                int firstQuote = line.IndexOf('"', colon + 1);
                int secondQuote = line.IndexOf('"', firstQuote + 1);
                if (firstQuote >= 0 && secondQuote > firstQuote)
                    return line.Substring(firstQuote + 1, secondQuote - firstQuote - 1);
            }
        }
        catch (Exception e)
        {
            Debug.LogWarning("[PhysicsSidecarExporter] SDK バージョンの読み取りに失敗した:" + e.Message);
        }
        return "unknown";
    }

    private static void ValidateSidecar(SidecarDto sidecar)
    {
        var nodeIds = new HashSet<string>(StringComparer.Ordinal);
        foreach (NodeDto node in sidecar.nodes) nodeIds.Add(node.id);
        var colliderIds = new HashSet<string>(StringComparer.Ordinal);
        foreach (ColliderDto collider in sidecar.colliders) colliderIds.Add(collider.id);

        foreach (PhysBoneDto bone in sidecar.physBones)
        {
            if (!nodeIds.Contains(bone.componentNodeId) || !nodeIds.Contains(bone.rootNodeId))
                throw new InvalidOperationException("PhysBone の参照ノードが nodes に無い:" + bone.id);
            foreach (string ignore in bone.ignoreNodeIds)
                if (!nodeIds.Contains(ignore))
                    throw new InvalidOperationException("ignoreTransforms の参照ノードが nodes に無い:" + bone.id);
            foreach (string colliderId in bone.colliderIds)
                if (!colliderIds.Contains(colliderId))
                    throw new InvalidOperationException("PhysBone の参照コライダーが colliders に無い:" + bone.id);
        }
        foreach (ColliderDto collider in sidecar.colliders)
        {
            if (!nodeIds.Contains(collider.componentNodeId) || !nodeIds.Contains(collider.rootNodeId))
                throw new InvalidOperationException("Collider の参照ノードが nodes に無い:" + collider.id);
        }
        foreach (NodeDto node in sidecar.nodes)
        {
            if (node.referenceSource != "bind"
                && node.referenceSource != "scene"
                && node.referenceSource != "bind-decomposed-approx")
                throw new InvalidOperationException("referenceSource が不正:" + node.id);
            if (node.source != null && (node.source.path == null || node.source.path.Length == 0))
                Debug.LogWarning("[PhysicsSidecarExporter] source.path が空:" + node.id);
        }
        if (sidecar.capture.statePolicy == "bind-pose"
            && (sidecar.capture.bindPoseSource == null || sidecar.capture.bindPoseSource.Length == 0))
            throw new InvalidOperationException("bind-pose の bindPoseSource が空。");
        foreach (SourceModelDto model in sidecar.sourceModels)
        {
            if (model.sha256 == null || model.sha256.Length != 64)
                throw new InvalidOperationException("SHA-256 が不正:" + model.id);
        }
    }

    private static float[] Float3(Vector3 v)
    {
        return new[] { v.x, v.y, v.z };
    }

    private static float[] Float4(Quaternion q)
    {
        return new[] { q.x, q.y, q.z, q.w };
    }

    // C# の null を JsonUtility が展開した残骸に戻す正規表現。
    // pretty 出力の空白ゆらぎだけ許し、内容は既定値の組み合わせに限定する。
    // いずれも正規の値では空にならない (ID は n:N 形式、modelId は非空) 前提。
    private static readonly Regex NullParentPattern =
        new Regex("\"parentId\": \"\"");

    private static readonly Regex NullSourcePattern = new Regex(
        "\"source\": \\{\\s*\"modelId\": \"\",\\s*\"path\": \\[\\],\\s*\"assetGuid\": \"\",\\s*\"localId\": 0\\s*\\}");

    // 以下、JsonUtility 用 DTO。フィールド順が JSON の順序になる。
    [Serializable]
    private sealed class SidecarDto
    {
        public string format;
        public int schemaVersion;
        public GeneratorDto generator;
        public CaptureDto capture;
        public SourceModelDto[] sourceModels;
        public NodeDto[] nodes;
        public PhysBoneDto[] physBones;
        public ColliderDto[] colliders;
        public string[] unsupportedFeatures;
    }

    [Serializable]
    private sealed class GeneratorDto
    {
        public string name;
        public string version;
        public string unityVersion;
        public string sdkVersion;
    }

    [Serializable]
    private sealed class CaptureDto
    {
        public string statePolicy;
        public string coordinateSpace;
        public string avatarRootId;
        public float[] rootScale;
        public string[] bindPoseSource;
    }

    [Serializable]
    private sealed class SourceModelDto
    {
        public string id;
        public string sha256;
        public string assetGuid;
        public string assetPath;
    }

    [Serializable]
    private sealed class NodeDto
    {
        public string id;
        public string parentId;
        public string name;
        public int siblingIndex;
        public string[] scenePath;
        public SourceRefDto source;
        public string referenceSource;
        public TransformDto referenceLocal;
    }

    [Serializable]
    private sealed class SourceRefDto
    {
        public string modelId;
        public string[] path;
        public string assetGuid;
        public long localId;
    }

    [Serializable]
    private sealed class TransformDto
    {
        public float[] position;
        public float[] rotation;
        public float[] scale;
    }

    [Serializable]
    private sealed class PhysBoneDto
    {
        public string id;
        public string componentNodeId;
        public string rootNodeId;
        public bool enabled;
        public string version;
        public string integrationType;
        public string[] ignoreNodeIds;
        public bool ignoreOtherPhysBones;
        public float[] endpointPosition;
        public string multiChildType;
        public CurvedDto pull;
        public CurvedDto spring;
        public CurvedDto stiffness;
        public CurvedDto gravity;
        public CurvedDto gravityFalloff;
        public string immobileType;
        public CurvedDto immobile;
        public CurvedDto radius;
        public bool allowCollision;
        public string[] colliderIds;
        public string limitType;
        public CurvedDto maxAngleX;
        public CurvedDto maxAngleZ;
        public float[] limitRotation;
        public CurvedDto limitRotationXCurve;
        public CurvedDto limitRotationYCurve;
        public CurvedDto limitRotationZCurve;
        public bool isAnimated;
    }

    [Serializable]
    private sealed class ColliderDto
    {
        public string id;
        public string componentNodeId;
        public string rootNodeId;
        public bool enabled;
        public string shapeType;
        public bool insideBounds;
        public float radius;
        public float height;
        public float[] position;
        public float[] rotation;
        public bool bonesAsSpheres;
    }

    [Serializable]
    private sealed class CurvedDto
    {
        public float value;
        public CurveKeyDto[] keys;
        public int preWrapMode;
        public int postWrapMode;
    }

    [Serializable]
    private sealed class CurveKeyDto
    {
        public float time;
        public float value;
        public float inSlope;
        public float outSlope;
        public int weightedMode;
        public float inWeight;
        public float outWeight;
    }
}
