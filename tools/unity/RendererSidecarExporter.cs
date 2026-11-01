// vrctom RendererSidecarExporter
// アバター Prefab を空シーンへ実体化し、Unity が解決した材質割当と
// 出力対象レンダラを sidecar JSON (format: vrctom.renderers, schemaVersion: 1) に書き出す。
// Rust 側が材質割当と出力対象レンダラを正確に再現するための入力である。
// シーンや Prefab を保存・変更しない。edit-mode の初期状態を採取する
// (Play Mode・Animator・VRChat 実行時は対象外)。
using System;
using System.Collections.Generic;
using System.IO;
using System.Security.Cryptography;
using System.Text;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;

/// <summary>
/// レンダラ sidecar JSON を書き出す。バッチ実行では
/// -executeMethod RendererSidecarExporter.Run で呼び出す。
/// </summary>
public static class RendererSidecarExporter
{
    private const string FormatName = "vrctom.renderers";
    private const int SchemaVersion = 1;

    /// <summary>GUI メニューからの実行口。環境変数に従い、エディタは終了しない。</summary>
    [MenuItem("Tools/vrctom/Export Renderer Sidecar")]
    public static void ExportFromMenu()
    {
        Export(failQuit: false);
    }

    /// <summary>-executeMethod 用の実行口。環境変数に従い、成功で 0、失敗で 1 を返して終了する。</summary>
    public static void Run()
    {
        Export(failQuit: true);
    }

    private static void Export(bool failQuit)
    {
        string prefabPath = Environment.GetEnvironmentVariable("VRCTOM_RENDERERS_PREFAB");
        string outPath = Environment.GetEnvironmentVariable("VRCTOM_RENDERERS_OUT");
        if (string.IsNullOrEmpty(prefabPath) || string.IsNullOrEmpty(outPath))
        {
            Debug.LogError("[RendererSidecarExporter] VRCTOM_RENDERERS_PREFAB と VRCTOM_RENDERERS_OUT が必要。");
            if (Application.isBatchMode || failQuit) EditorApplication.Exit(2);
            return;
        }

        try
        {
            var prefab = AssetDatabase.LoadAssetAtPath<GameObject>(prefabPath);
            if (prefab == null)
                throw new InvalidOperationException("prefab を読めない:" + prefabPath);

            // 空シーンへ実体化し、edit-mode の初期状態を採取する。シーンは保存しない。
            Scene scene = EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
            var instance = (GameObject)PrefabUtility.InstantiatePrefab(prefab, scene);
            if (instance == null)
                throw new InvalidOperationException("プレハブを実体化できない:" + prefabPath);

            string json = BuildSidecar(prefabPath, instance);
            string directory = Path.GetDirectoryName(outPath);
            if (!string.IsNullOrEmpty(directory)) Directory.CreateDirectory(directory);
            // BOM 無し UTF-8 で書き出す。Rust 側の読み取りと差分比較を安定させる。
            File.WriteAllText(outPath, json, new UTF8Encoding(false));

            // シーンは保存せずに捨てる。
            EditorSceneManager.NewScene(NewSceneSetup.EmptyScene, NewSceneMode.Single);
            Debug.Log("[RendererSidecarExporter] wrote " + outPath);
            if (Application.isBatchMode) EditorApplication.Exit(0);
        }
        catch (Exception e)
        {
            Debug.LogError("[RendererSidecarExporter] " + e.Message + "\n" + e.StackTrace);
            if (Application.isBatchMode || failQuit) EditorApplication.Exit(1);
        }
    }

    private static string BuildSidecar(string prefabPath, GameObject instance)
    {
        // 並びはシーン走査順 (GetComponentsInChildren の階層順) で固定する。
        Renderer[] renderers = instance.GetComponentsInChildren<Renderer>(true);

        var records = new List<RendererRecord>(renderers.Length);
        var models = new SortedDictionary<string, ModelEntry>(StringComparer.Ordinal);
        int supportedCount = 0;
        int nullModelCount = 0;
        for (int i = 0; i < renderers.Length; i++)
        {
            RendererRecord record = BuildRecord(i, instance.transform, renderers[i], models);
            records.Add(record);
            if (record.Supported) supportedCount++;
            if (record.Model == null) nullModelCount++;
        }

        var sb = new StringBuilder();
        sb.Append("{\n");
        sb.Append("  \"format\": ").Append(Json(FormatName)).Append(",\n");
        sb.Append("  \"schemaVersion\": ").Append(SchemaVersion).Append(",\n");
        sb.Append("  \"unityVersion\": ").Append(Json(Application.unityVersion)).Append(",\n");
        sb.Append("  \"sourcePrefab\": {\"path\": ").Append(Json(prefabPath));
        sb.Append(", \"guid\": ").Append(Json(AssetDatabase.AssetPathToGUID(prefabPath))).Append("},\n");

        sb.Append("  \"models\": [");
        bool firstModel = true;
        foreach (ModelEntry model in models.Values)
        {
            if (!firstModel) sb.Append(", ");
            firstModel = false;
            sb.Append("{\"path\": ").Append(Json(model.Path));
            sb.Append(", \"guid\": ").Append(Json(model.Guid));
            sb.Append(", \"sha256\": ").Append(Json(model.Sha256)).Append("}");
        }
        sb.Append("],\n");

        sb.Append("  \"renderers\": [\n");
        for (int i = 0; i < records.Count; i++)
        {
            if (i > 0) sb.Append(",\n");
            AppendRenderer(sb, records[i]);
        }
        sb.Append("\n  ]\n}\n");

        Debug.Log(string.Format(
            "[RendererSidecarExporter] renderers={0} supported={1} nullModel={2} models={3}",
            records.Count, supportedCount, nullModelCount, models.Count));
        return sb.ToString();
    }

    // レンダラ1件分の採取結果。JSON へ出す直前の形であり、models への登録は済んでいる。
    private sealed class RendererRecord
    {
        public int Index;
        public string RootPath;
        public string Type;
        public bool Supported;
        public bool Enabled;
        public bool ActiveInHierarchy;
        public ModelRef Model;
        public string ModelNote;
        public Mesh Mesh;
        public int BoneCount;
        public Material[] Materials;
        public string UnsupportedNote;
    }

    private sealed class ModelRef
    {
        public string Path;
        public string Guid;
        public string NodePath;
    }

    private sealed class ModelEntry
    {
        public string Path;
        public string Guid;
        public string Sha256;
    }

    private static RendererRecord BuildRecord(
        int index, Transform root, Renderer renderer, SortedDictionary<string, ModelEntry> models)
    {
        var record = new RendererRecord
        {
            Index = index,
            RootPath = PathOf(root, renderer.transform),
            Type = renderer.GetType().Name,
            Supported = renderer is SkinnedMeshRenderer || renderer is MeshRenderer,
            Enabled = renderer.enabled,
            ActiveInHierarchy = renderer.gameObject.activeInHierarchy,
            Materials = renderer.sharedMaterials,
        };

        if (!record.Supported)
        {
            // 対応しない種類も黙って飛ばさず、存在だけ記録する。
            record.UnsupportedNote = "unsupported renderer type: " + record.Type;
            return record;
        }

        var smr = renderer as SkinnedMeshRenderer;
        if (smr != null)
        {
            record.Mesh = smr.sharedMesh;
            record.BoneCount = smr.bones != null ? smr.bones.Length : 0;
        }
        else
        {
            // MeshRenderer のメッシュは MeshFilter が持つ。
            var filter = renderer.GetComponent<MeshFilter>();
            record.Mesh = filter != null ? filter.sharedMesh : null;
            record.BoneCount = 0;
        }

        record.Model = ResolveModel(renderer.transform, out record.ModelNote);
        if (record.Model != null) RegisterModel(models, record.Model.Path);
        if (record.Mesh != null) RegisterMeshModel(models, record.Mesh);

        return record;
    }

    // レンダラの GameObject から親をたどり、どのモデル (FBX アセット) に属するかを判定する。
    // そのモデルのルート (Unity が FBX から生成するルート GameObject) からの相対パスを返す。
    // Rust 側は FBX 内のノードパスでメッシュを照合するため、Unity の fileID では照合しない。
    // ルートが特定できない場合は null を返し、推測したパスを出さない。
    private static ModelRef ResolveModel(Transform node, out string note)
    {
        note = null;

        // シーン→Prefab→FBX と対応元を遡り、最も外側の FBX 由来オブジェクトを使う。
        // Prefab YAML からは解決できない形 (FBX 本体を source にする PrefabInstance) も
        // Unity の解決結果として受け取るための遡及である。
        Transform src = node;
        while (true)
        {
            var outer = PrefabUtility.GetCorrespondingObjectFromSource(src) as Transform;
            if (outer == null) break;
            src = outer;
        }
        if (src == node)
        {
            note = "node has no prefab/FBX source (scene or prefab-local node)";
            return null;
        }

        string assetPath = AssetDatabase.GetAssetPath(src);
        if (string.IsNullOrEmpty(assetPath) || !assetPath.EndsWith(".fbx", StringComparison.OrdinalIgnoreCase))
        {
            note = "outermost source is not an FBX model"
                + (string.IsNullOrEmpty(assetPath) ? " (no asset path)" : " (" + assetPath + ")");
            return null;
        }

        // 同じアセットの親を辿り、最も外側 (FBX ルート) の手前までを集める。
        Transform fbxRoot = src;
        while (fbxRoot.parent != null && AssetDatabase.GetAssetPath(fbxRoot.parent) == assetPath)
            fbxRoot = fbxRoot.parent;

        var names = new List<string>();
        for (Transform s = src; s != null && s != fbxRoot; s = s.parent) names.Add(s.name);
        if (names.Count == 0)
        {
            // 自身が FBX ルートの場合、ルート基準のパスは空になる。空 path を出さない。
            note = "renderer node is the FBX root; no node-local path";
            return null;
        }
        names.Reverse();

        string guid = AssetDatabase.AssetPathToGUID(assetPath);
        return new ModelRef { Path = assetPath, Guid = guid, NodePath = string.Join("/", names) };
    }

    private static void RegisterModel(SortedDictionary<string, ModelEntry> models, string assetPath)
    {
        if (models.ContainsKey(assetPath)) return;
        models[assetPath] = new ModelEntry
        {
            Path = assetPath,
            Guid = AssetDatabase.AssetPathToGUID(assetPath),
            Sha256 = Sha256OfAsset(assetPath),
        };
    }

    // メッシュの FBX が所有ノードの FBX と異なる場合に備え、メッシュ側の FBX も models へ入れる。
    private static void RegisterMeshModel(SortedDictionary<string, ModelEntry> models, Mesh mesh)
    {
        string assetPath = AssetDatabase.GetAssetPath(mesh);
        if (string.IsNullOrEmpty(assetPath) || !assetPath.EndsWith(".fbx", StringComparison.OrdinalIgnoreCase))
            return;
        RegisterModel(models, assetPath);
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

    private static void AppendRenderer(StringBuilder sb, RendererRecord record)
    {
        sb.Append("    {\n");
        sb.Append("      \"index\": ").Append(record.Index).Append(",\n");
        // rootPath はインスタンス基準の絶対パスであり、path と同じ値である。
        // Prefab 内の複数ルートに対応するため、インスタンスルート基準で統一する。
        sb.Append("      \"rootPath\": ").Append(Json(record.RootPath)).Append(",\n");
        sb.Append("      \"path\": ").Append(Json(record.RootPath)).Append(",\n");
        sb.Append("      \"type\": ").Append(Json(record.Type)).Append(",\n");
        sb.Append("      \"supported\": ").Append(record.Supported ? "true" : "false").Append(",\n");
        sb.Append("      \"enabled\": ").Append(record.Enabled ? "true" : "false").Append(",\n");
        sb.Append("      \"activeInHierarchy\": ").Append(record.ActiveInHierarchy ? "true" : "false").Append(",\n");

        if (record.Model == null)
            sb.Append("      \"model\": null,\n");
        else
            sb.Append("      \"model\": {\"path\": ").Append(Json(record.Model.Path))
                .Append(", \"guid\": ").Append(Json(record.Model.Guid))
                .Append(", \"nodePath\": ").Append(Json(record.Model.NodePath)).Append("},\n");

        if (record.Mesh == null)
            sb.Append("      \"mesh\": null,\n");
        else
            sb.Append("      \"mesh\": ").Append(MeshJson(record.Mesh, record.BoneCount)).Append(",\n");

        sb.Append("      \"materials\": [");
        if (record.Supported)
        {
            for (int i = 0; i < record.Materials.Length; i++)
            {
                if (i > 0) sb.Append(", ");
                sb.Append(record.Materials[i] == null ? "null" : MaterialJson(record.Materials[i]));
            }
        }
        sb.Append("],\n");

        var notes = new List<string>(2);
        if (record.UnsupportedNote != null) notes.Add(record.UnsupportedNote);
        if (record.ModelNote != null) notes.Add(record.ModelNote);
        if (record.Supported && record.Mesh == null) notes.Add("sharedMesh is null");
        sb.Append("      \"note\": ").Append(notes.Count == 0 ? "null" : Json(string.Join("; ", notes.ToArray())));
        sb.Append("\n    }");
    }

    private static string MeshJson(Mesh mesh, int boneCount)
    {
        var sb = new StringBuilder();
        sb.Append("{\"name\": ").Append(Json(mesh.name));
        sb.Append(", \"path\": ").Append(Json(AssetDatabase.GetAssetPath(mesh)));
        AppendGuidAndFileId(sb, mesh);
        sb.Append(", \"subMeshCount\": ").Append(mesh.subMeshCount);
        sb.Append(", \"vertexCount\": ").Append(mesh.vertexCount);
        sb.Append(", \"boneCount\": ").Append(boneCount);
        sb.Append("}");
        return sb.ToString();
    }

    private static string MaterialJson(Material material)
    {
        var sb = new StringBuilder();
        sb.Append("{\"name\": ").Append(Json(material.name));
        sb.Append(", \"guid\": ").Append(Json(MaterialGuid(material)));
        sb.Append(", \"fileID\": ").Append(MaterialFileId(material));
        sb.Append(", \"path\": ").Append(Json(AssetDatabase.GetAssetPath(material)));
        sb.Append(", \"shader\": ").Append(Json(material.shader == null ? "<null>" : material.shader.name));
        sb.Append("}");
        return sb.ToString();
    }

    // TryGetGUIDAndLocalFileIdentifier が失敗する組み込み材質等は guid 空・fileID 0 で出す。
    private static string MaterialGuid(Material material)
    {
        string guid;
        long localId;
        if (AssetDatabase.TryGetGUIDAndLocalFileIdentifier(material, out guid, out localId)) return guid;
        return string.Empty;
    }

    private static string MaterialFileId(Material material)
    {
        string guid;
        long localId;
        if (AssetDatabase.TryGetGUIDAndLocalFileIdentifier(material, out guid, out localId))
            return localId.ToString();
        return "0";
    }

    private static void AppendGuidAndFileId(StringBuilder sb, UnityEngine.Object asset)
    {
        string guid;
        long localId;
        if (AssetDatabase.TryGetGUIDAndLocalFileIdentifier(asset, out guid, out localId))
        {
            sb.Append(", \"guid\": ").Append(Json(guid));
            // 64bit の fileID は丸めない。浮動小数や指数表記にしない。
            sb.Append(", \"fileID\": ").Append(localId.ToString());
        }
        else
        {
            sb.Append(", \"guid\": \"\", \"fileID\": 0");
        }
    }

    private static string PathOf(Transform root, Transform target)
    {
        var parts = new List<string>();
        for (Transform current = target; current != null && current != root; current = current.parent)
            parts.Add(current.name);
        parts.Reverse();
        return string.Join("/", parts);
    }

    private static string Json(string value)
    {
        if (value == null) return "null";
        var sb = new StringBuilder("\"");
        foreach (char c in value)
        {
            switch (c)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                default:
                    if (c < 0x20) sb.Append("\\u").Append(((int)c).ToString("x4"));
                    else sb.Append(c);
                    break;
            }
        }
        sb.Append('"');
        return sb.ToString();
    }
}
