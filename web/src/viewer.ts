/**
 * VRMのプレビュー表示。表示・回転・ズームだけを受け持つ。
 *
 * 物理の再生や表情の切り替えは行わない。読み込みの失敗は呼び出し側へ投げ、
 * ダウンロードの可否には触れない。
 */
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { VRM, VRMLoaderPlugin, VRMUtils } from "@pixiv/three-vrm";

/**
 * 描画域の持ち主。canvasの寿命より短く使い、終わったら`dispose`する。
 *
 * 新しいモデルの読み込み前に旧モデルを破棄し、GPU資源を解放する。
 */
export class PreviewViewer {
  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene: THREE.Scene;
  private readonly camera: THREE.PerspectiveCamera;
  private readonly controls: OrbitControls;
  private readonly grid: THREE.GridHelper;
  private readonly clock = new THREE.Clock();
  private readonly resizeObserver: ResizeObserver;
  private currentVrm: VRM | null = null;
  /** 読み込み要求の世代。古い読み込みの遅延完了を捨てるために使う。 */
  private loadGeneration = 0;
  private animationId = 0;
  private disposed = false;

  /**
   * @param canvas - 描画先のcanvas。大きさは親要素に追従する。
   */
  constructor(canvas: HTMLCanvasElement) {
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = 1.0;
    // ページの背景と合わせた落ち着いた暗色。
    this.renderer.setClearColor(new THREE.Color(0x1a1d24));

    this.scene = new THREE.Scene();
    this.camera = new THREE.PerspectiveCamera(45, 1, 0.01, 100);
    this.camera.position.set(0.6, 1.4, 2.2);

    const hemisphere = new THREE.HemisphereLight(0xffffff, 0x3a3f4a, 1.0);
    const directional = new THREE.DirectionalLight(0xffffff, 1.6);
    directional.position.set(2, 4, 3);
    const ambient = new THREE.AmbientLight(0xffffff, 0.4);
    this.scene.add(hemisphere, directional, ambient);

    // モデルの足元に薄く敷く目安。読み込みごとに位置と広さを寄せる。
    this.grid = new THREE.GridHelper(2, 20, 0x4a5160, 0x2e3340);
    const gridMaterial = this.grid.material;
    if (Array.isArray(gridMaterial)) {
      for (const entry of gridMaterial) {
        entry.transparent = true;
        entry.opacity = 0.5;
      }
    } else {
      gridMaterial.transparent = true;
      gridMaterial.opacity = 0.5;
    }
    this.scene.add(this.grid);

    this.controls = new OrbitControls(this.camera, this.renderer.domElement);
    this.controls.enableDamping = true;
    this.controls.target.set(0, 1, 0);

    const parent = canvas.parentElement;
    this.resizeObserver = new ResizeObserver(() => {
      this.resize();
    });
    if (parent !== null) {
      this.resizeObserver.observe(parent);
    }
    this.resize();

    this.animationId = requestAnimationFrame(this.tick);
  }

  /**
   * VRMバイト列を読み込んで表示する。旧モデルは破棄する。
   *
   * 読み込み要求ごとに世代を進め、古い読み込みが遅れて終わっても
   * 現在の表示を変えない。遅れて読み込んだ場面は破棄する。
   *
   * @param data - VRM 1.0のGLBバイト列。複写せず読み取る。
   * @throws `Error` — 最新の読み込みに失敗した場合。旧モデルは破棄済みである。
   */
  async loadVrm(data: ArrayBuffer): Promise<void> {
    const generation = this.loadGeneration + 1;
    this.loadGeneration = generation;
    const loader = new GLTFLoader();
    loader.register((parser) => new VRMLoaderPlugin(parser));
    let vrm: VRM;
    try {
      const gltf = await loader.parseAsync(data, "");
      // `userData`は型上`any`のため、`unknown`に受けてから絞り込む。
      const loaded: unknown = gltf.userData.vrm;
      if (!(loaded instanceof VRM)) {
        throw new Error("VRMの読み込みに失敗した");
      }
      vrm = loaded;
    } catch (error) {
      // 古い世代の失敗では現在の表示に触れず、呼び出し側の表示も壊さない。
      if (this.loadGeneration !== generation) {
        return;
      }
      this.clearModel();
      if (error instanceof Error) {
        throw error;
      }
      throw new Error("VRMの読み込みに失敗した");
    }
    // 世代が変わっていたら遅れて終わった読み込みであり、現在の表示を変えず破棄する。
    if (this.loadGeneration !== generation) {
      VRMUtils.deepDispose(vrm.scene);
      return;
    }
    this.clearModel();
    this.currentVrm = vrm;
    this.scene.add(vrm.scene);
    this.frameModel(vrm.scene);
  }

  /** 表示中のモデルを破棄し、空の描画域に戻す。進行中の読み込みも無効化する。 */
  clear(): void {
    this.loadGeneration += 1;
    this.clearModel();
  }

  /** 後片付け。レンダーループと監視を止め、GPU資源を解放する。 */
  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    // 進行中の読み込みを無効化する。
    this.loadGeneration += 1;
    cancelAnimationFrame(this.animationId);
    this.resizeObserver.disconnect();
    this.clearModel();
    this.controls.dispose();
    this.renderer.dispose();
  }

  /** 旧モデルを場面から外して破棄する。 */
  private clearModel(): void {
    if (this.currentVrm === null) {
      return;
    }
    this.scene.remove(this.currentVrm.scene);
    VRMUtils.deepDispose(this.currentVrm.scene);
    this.currentVrm = null;
  }

  /**
   * モデル全体が収まる向きにカメラと注視点を寄せる。
   *
   * @param model - 枠を測る対象。
   */
  private frameModel(model: THREE.Object3D): void {
    const box = new THREE.Box3().setFromObject(model);
    if (box.isEmpty()) {
      return;
    }
    const center = box.getCenter(new THREE.Vector3());
    const size = box.getSize(new THREE.Vector3());
    const largest = Math.max(size.x, size.y, size.z);
    if (!(largest > 0)) {
      return;
    }
    // 視野角45度で全体が収まる目安の距離。係数は見た目の余白である。
    const distance = largest * 2.2;
    this.controls.target.copy(center);
    this.camera.position.set(center.x + distance * 0.3, center.y + distance * 0.12, center.z + distance);
    this.camera.near = distance / 100;
    this.camera.far = distance * 100;
    this.camera.updateProjectionMatrix();
    this.grid.position.y = box.min.y;
    const gridSize = Math.max(largest * 2, 0.5);
    this.grid.scale.set(gridSize / 2, 1, gridSize / 2);
    this.controls.update();
  }

  /** 描画域を親要素の大きさに合わせる。 */
  private resize(): void {
    const parent = this.renderer.domElement.parentElement;
    if (parent === null) {
      return;
    }
    const width = parent.clientWidth;
    const height = parent.clientHeight;
    if (width <= 0 || height <= 0) {
      return;
    }
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.setSize(width, height);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
  }

  /** 毎フレームの更新。VRMの時間進行と描画を行う。 */
  private readonly tick = (): void => {
    if (this.disposed) {
      return;
    }
    this.animationId = requestAnimationFrame(this.tick);
    const delta = Math.min(this.clock.getDelta(), 0.1);
    this.currentVrm?.update(delta);
    this.controls.update();
    this.renderer.render(this.scene, this.camera);
  };
}
