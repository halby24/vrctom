//! UnityPackage展開の契約テスト（公開 API のみを使う）。
//!
//! 小さなパッケージはテスト内で生成し、販売物fixtureやその抜粋は置かない。
//! 実fixtureを使う計測テストは `VRCTOM_UNITYPACKAGE_FIXTURE` が指す
//! パッケージだけを読み、未設定なら跳ばす。

#![cfg(not(target_arch = "wasm32"))]

#[cfg(unix)]
use std::collections::HashSet;
use std::fs;
#[cfg(unix)]
use std::fs::File;
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
#[cfg(unix)]
use tar::Archive;
use vrctom::unity::{AssetKind, AssetSource, IndexOptions, UnityErrorKind};
use vrctom::unitypackage::{
    EntryKind, UnpackErrorKind, UnpackOptions, UnpackReport, unpack_in_memory, unpack_to,
};

// テスト名は日本語で書く（テストは仕様・契約であるため）.

/// テスト用メンバー。順序は `Vec` の並びでtarへ書き込む。
enum Member {
    Dir {
        guid: String,
    },
    Pathname {
        guid: String,
        content: Vec<u8>,
    },
    Asset {
        guid: String,
        content: Vec<u8>,
    },
    Meta {
        guid: String,
        content: Vec<u8>,
    },
    Preview {
        guid: String,
        content: Vec<u8>,
    },
    /// ヘッダ直接組み立ての壊れた・特殊なエントリ。
    Raw {
        path: Vec<u8>,
        typeflag: u8,
        link: Vec<u8>,
        content: Vec<u8>,
    },
}

/// 32桁ASCII hexのテスト用GUIDを生成する。
fn guid(n: u128) -> String {
    format!("{n:032x}")
}

/// tarの1ブロックを組み立てる。通常のustar配置でチェックサムを付ける。
fn raw_block(name: &[u8], typeflag: u8, size: u64, linkname: &[u8]) -> [u8; 512] {
    let mut block = [0u8; 512];
    let name_len = name.len().min(100);
    block[..name_len].copy_from_slice(&name[..name_len]);
    block[100..108].copy_from_slice(b"0000777\0");
    block[108..116].copy_from_slice(b"0000000\0");
    block[116..124].copy_from_slice(b"0000000\0");
    let size_field = format!("{size:011o}\0");
    block[124..136].copy_from_slice(size_field.as_bytes());
    block[136..148].copy_from_slice(b"00000000000\0");
    block[148..156].copy_from_slice(b"        ");
    block[156] = typeflag;
    let link_len = linkname.len().min(100);
    block[157..157 + link_len].copy_from_slice(&linkname[..link_len]);
    block[257..262].copy_from_slice(b"ustar");
    block[263..265].copy_from_slice(b"00");
    let sum: u32 = block.iter().map(|byte| *byte as u32).sum();
    let cksum_field = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(cksum_field.as_bytes());
    block
}

/// メンバー列をtarバイト列にする。終端のゼロ2ブロックまで含める。
fn tar_bytes(members: &[Member]) -> Vec<u8> {
    let mut out = Vec::new();
    for member in members {
        let (path, typeflag, link, content): (Vec<u8>, u8, Vec<u8>, &[u8]) = match member {
            Member::Dir { guid } => (format!("{guid}/").into_bytes(), b'5', Vec::new(), &[]),
            Member::Pathname { guid, content } => (
                format!("{guid}/pathname").into_bytes(),
                b'0',
                Vec::new(),
                content,
            ),
            Member::Asset { guid, content } => (
                format!("{guid}/asset").into_bytes(),
                b'0',
                Vec::new(),
                content,
            ),
            Member::Meta { guid, content } => (
                format!("{guid}/asset.meta").into_bytes(),
                b'0',
                Vec::new(),
                content,
            ),
            Member::Preview { guid, content } => (
                format!("{guid}/preview.png").into_bytes(),
                b'0',
                Vec::new(),
                content,
            ),
            Member::Raw {
                path,
                typeflag,
                link,
                content,
            } => (path.clone(), *typeflag, link.clone(), content),
        };
        out.extend_from_slice(&raw_block(&path, typeflag, content.len() as u64, &link));
        out.extend_from_slice(content);
        out.extend_from_slice(&vec![0u8; (512 - content.len() % 512) % 512]);
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

/// メンバー列を `.unitypackage` バイト列（gzip tar）にする。
fn package_bytes(members: &[Member]) -> Vec<u8> {
    let tar = tar_bytes(members);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    encoder.finish().expect("gzipは完成する")
}

/// パッケージを一時親の下の `out` へ展開する。
fn unpack_fixture(
    pkg: &[u8],
    options: &UnpackOptions,
) -> (
    tempfile::TempDir,
    PathBuf,
    Result<UnpackReport, vrctom::unitypackage::UnpackError>,
) {
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let destination = parent.path().join("out");
    let result = unpack_to(pkg, &destination, options);
    (parent, destination, result)
}

/// ファイル1件とフォルダ1件の基本パッケージを作る。
fn basic_members() -> Vec<Member> {
    vec![
        Member::Dir { guid: guid(1) },
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"hello".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"meta-a".to_vec(),
        },
        Member::Dir { guid: guid(2) },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/Empty".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"meta-empty".to_vec(),
        },
    ]
}

#[test]
fn ファイルと空ファイルと空フォルダの内容が一致する() {
    let members = vec![
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"hello".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"meta-a".to_vec(),
        },
        Member::Preview {
            guid: guid(1),
            content: b"preview-bytes".to_vec(),
        },
        // 空の `asset` は空ファイルとして出力する。
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/empty.bin".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: Vec::new(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"".to_vec(),
        },
        // `asset` の無いGUIDはフォルダになる。
        Member::Dir { guid: guid(3) },
        Member::Pathname {
            guid: guid(3),
            content: b"Assets/Empty".to_vec(),
        },
        Member::Meta {
            guid: guid(3),
            content: b"meta-empty".to_vec(),
        },
    ];
    let pkg = package_bytes(&members);
    let (_parent, destination, result) = unpack_fixture(&pkg, &UnpackOptions::default());
    let report = result.expect("基本的な展開は成功する");
    assert_eq!(
        fs::read(destination.join("Assets/a.txt")).expect("出力は読める"),
        b"hello"
    );
    assert_eq!(
        fs::read(destination.join("Assets/a.txt.meta")).expect("出力は読める"),
        b"meta-a"
    );
    assert_eq!(
        fs::read(destination.join("Assets/empty.bin")).expect("出力は読める"),
        Vec::<u8>::new()
    );
    assert!(destination.join("Assets/Empty").is_dir());
    assert_eq!(
        fs::read(destination.join("Assets/Empty.meta")).expect("出力は読める"),
        b"meta-empty"
    );
    // `preview.png` は出力しない。
    assert!(!destination.join("Assets/preview.png").exists());
    assert_eq!(report.file_count(), 2);
    assert_eq!(report.directory_count(), 1);
    assert_eq!(
        report.output_bytes(),
        b"hello".len() as u64 + b"meta-a".len() as u64 + b"meta-empty".len() as u64
    );
    // 一覧は `pathname` のUTF-8バイト列順である。
    let paths: Vec<&str> = report
        .entries()
        .iter()
        .map(|entry| entry.pathname.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["Assets/Empty", "Assets/a.txt", "Assets/empty.bin"]
    );
    assert_eq!(report.entries()[1].kind, EntryKind::File);
    assert_eq!(report.entries()[0].kind, EntryKind::Directory);
    assert_eq!(report.entries()[0].guid, guid(3));
}

#[test]
fn メモリ展開でmetaを相対パスで読める() {
    // `asset` と `asset.meta` の両方を相対パスから実際の出力パスへの表に登録し、
    // native の読み手と同じく `.meta` を読めることの回帰として残す。
    // ファイルmetaとフォルダmetaの両方で確かめる。
    let members = vec![
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/Model.fbx".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"fbx-body".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"fbx-meta".to_vec(),
        },
        Member::Dir { guid: guid(2) },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/Empty".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"empty-meta".to_vec(),
        },
    ];
    let package = unpack_in_memory(&package_bytes(&members)[..], &UnpackOptions::default())
        .expect("展開できる");
    for (全体, 相対) in [
        ("Assets/Model.fbx.meta", "Model.fbx.meta"),
        ("Assets/Empty.meta", "Empty.meta"),
    ] {
        let 全文 = package.read(全体).expect("metaはある");
        let 読めた = package
            .read_limited(相対, 1024)
            .expect("相対パスでmetaを読める");
        assert_eq!(読めた, 全文, "同じバイト列である: {相対}");
    }
}

#[test]
fn assetがpathnameより先でもguid間で交錯しても展開できる() {
    // 対応付けを全件読み終えてから行うため、順序に依存しない。
    let members = vec![
        Member::Asset {
            guid: guid(1),
            content: b"late-path".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"second".to_vec(),
        },
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/first.txt".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/second.txt".to_vec(),
        },
    ];
    let pkg = package_bytes(&members);
    let (_parent, destination, result) = unpack_fixture(&pkg, &UnpackOptions::default());
    result.expect("順序違いでも成功する");
    assert_eq!(
        fs::read(destination.join("Assets/first.txt")).expect("出力は読める"),
        b"late-path"
    );
    assert_eq!(
        fs::read(destination.join("Assets/second.txt")).expect("出力は読める"),
        b"second"
    );
}

#[test]
fn 同一guidの同一メンバー再出現は失敗する() {
    // `asset` の重複。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"one".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"two".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("asset重複は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
    assert_eq!(error.guid(), Some(guid(1).as_str()));

    // `pathname` の重複。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("pathname重複は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);

    // GUIDディレクトリの重複。
    let pkg = package_bytes(&[
        Member::Dir { guid: guid(1) },
        Member::Dir { guid: guid(1) },
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("GUIDディレクトリ重複は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn 別guidの同一pathnameは内容同一でも失敗する() {
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/same.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"same".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"same".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/same.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"same".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"same".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("別GUIDの同一pathnameは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
}

#[test]
fn metaの出力先衝突は失敗する() {
    // `Assets/x` のmetaと `Assets/x.meta` の実体が同じ出力先になる。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/x".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"one".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/x.meta".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"two".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("meta衝突は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
}

#[test]
fn ファイルとディレクトリの衝突は失敗する() {
    // `Assets/a` がファイルかつ `Assets/a/b` の親になる。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"file".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/a/b.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"nested".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("親がファイルになる構造は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
}

#[test]
fn 必須欠損は失敗する() {
    // `pathname` 欠損。
    let pkg = package_bytes(&[
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("pathname欠損は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
    assert_eq!(error.guid(), Some(guid(1).as_str()));

    // `asset.meta` 欠損。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("meta欠損は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn asset不在はフォルダになりmetaは拡張子付きで出る() {
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/Folder".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"folder-meta".to_vec(),
        },
    ]);
    let (_parent, destination, result) = unpack_fixture(&pkg, &UnpackOptions::default());
    let report = result.expect("フォルダ展開は成功する");
    assert_eq!(report.entries().len(), 1);
    assert_eq!(report.entries()[0].kind, EntryKind::Directory);
    assert!(destination.join("Assets/Folder").is_dir());
    assert_eq!(
        fs::read(destination.join("Assets/Folder.meta")).expect("metaは読める"),
        b"folder-meta"
    );
}

#[test]
fn 不正な出力パスは補正せず失敗する() {
    // `workspace` の検証と `Assets` 始まりの条件に触れる代表形を集める。
    let invalid = [
        "/absolute.txt",
        "Assets/../escape.txt",
        "Assets//double.txt",
        "Assets/trailing/",
        "back\\slash.txt",
        "C:/drive.txt",
        "Assets/colon:name.txt",
        "Assets/less<than.txt",
        "Assets/CON.txt",
        "Assets/aux.dat",
        "Assets/COM1",
        "Assets/trailing-space /x.txt",
        "Assets/x.txt ",
        "Assets/x.txt.",
        "Assets/nul\0char.txt",
        "Assets/control\x1F.txt",
        "Assets/line\n.txt",
        "Assets",
    ];
    for pathname in invalid {
        // `Assets` そのものはファイルの場合だけ不正とするため、asset付きで試す。
        let pkg = package_bytes(&[
            Member::Pathname {
                guid: guid(1),
                content: pathname.as_bytes().to_vec(),
            },
            Member::Asset {
                guid: guid(1),
                content: b"x".to_vec(),
            },
            Member::Meta {
                guid: guid(1),
                content: b"m".to_vec(),
            },
        ]);
        let error = unpack_fixture(&pkg, &UnpackOptions::default())
            .2
            .expect_err(&format!("不正パスは失敗する: {pathname:?}"));
        assert_eq!(
            error.kind(),
            UnpackErrorKind::InvalidPath,
            "pathname: {pathname:?}"
        );
        assert_eq!(error.pathname(), Some(pathname));
    }
    // 非UTF-8の `pathname` も拒否する。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/\xff\xfe.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("非UTF-8は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPath);
}

#[test]
fn assets外の出力パスは失敗する() {
    for pathname in ["Packages/foo.prefab", "Other/x.txt", "Assets2/x.txt"] {
        let pkg = package_bytes(&[
            Member::Pathname {
                guid: guid(1),
                content: pathname.as_bytes().to_vec(),
            },
            Member::Asset {
                guid: guid(1),
                content: b"x".to_vec(),
            },
            Member::Meta {
                guid: guid(1),
                content: b"m".to_vec(),
            },
        ]);
        let error = unpack_fixture(&pkg, &UnpackOptions::default())
            .2
            .expect_err(&format!("Assets外は失敗する: {pathname:?}"));
        assert_eq!(
            error.kind(),
            UnpackErrorKind::InvalidPath,
            "pathname: {pathname:?}"
        );
    }
}

#[test]
fn 先頭成分の大文字小文字違いは許容する() {
    // 先頭成分の大小文字は問わず `Assets` として受け付ける。
    // ただしASCII大小文字だけ異なる名前の混在は衝突とするため、
    // ケースごとに一貫した表記で試す。
    for root in ["assets", "ASSETS"] {
        let pkg = package_bytes(&[
            Member::Pathname {
                guid: guid(1),
                content: format!("{root}/lower.txt").into_bytes(),
            },
            Member::Asset {
                guid: guid(1),
                content: b"a".to_vec(),
            },
            Member::Meta {
                guid: guid(1),
                content: b"m1".to_vec(),
            },
            Member::Pathname {
                guid: guid(2),
                content: format!("{root}/sub/upper.txt").into_bytes(),
            },
            Member::Asset {
                guid: guid(2),
                content: b"b".to_vec(),
            },
            Member::Meta {
                guid: guid(2),
                content: b"m2".to_vec(),
            },
        ]);
        let (_parent, destination, result) = unpack_fixture(&pkg, &UnpackOptions::default());
        result.expect("大小文字違いのAssetsは成功する");
        assert_eq!(
            fs::read(destination.join(format!("{root}/lower.txt"))).expect("出力は読める"),
            b"a"
        );
    }
}

#[test]
fn 大小文字だけ異なる名前は衝突する() {
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/Foo.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"a".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/foo.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"b".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("ASCII大小文字衝突は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
}

#[test]
fn symlinkやhardlinkや特殊や未知の種別は失敗する() {
    // 通常ファイル(0)とディレクトリ(5)以外はすべて拒否する。
    // GNU・PAX等の拡張型(L/K/x/g/S)も含める。
    for typeflag in [
        b'1', b'2', b'3', b'4', b'6', b'7', b'L', b'K', b'x', b'g', b'S', b'Z',
    ] {
        let link = if typeflag == b'1' || typeflag == b'2' {
            b"Assets/target".as_slice()
        } else {
            b"".as_slice()
        };
        let pkg = package_bytes(&[Member::Raw {
            path: format!("{}/asset", guid(1)).into_bytes(),
            typeflag,
            link: link.to_vec(),
            content: b"x".to_vec(),
        }]);
        let error = unpack_fixture(&pkg, &UnpackOptions::default())
            .2
            .expect_err(&format!("種別は失敗する: 0x{typeflag:02x}"));
        assert_eq!(
            error.kind(),
            UnpackErrorKind::InvalidPackage,
            "typeflag: 0x{typeflag:02x}"
        );
    }
    // ディレクトリ位置の symlink も拒否する。
    let pkg = package_bytes(&[Member::Raw {
        path: format!("{}/", guid(1)).into_bytes(),
        typeflag: b'2',
        link: b"Assets/target".to_vec(),
        content: Vec::new(),
    }]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("GUID位置のsymlinkは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn gzipのcrc不一致は失敗する() {
    let mut pkg = package_bytes(&basic_members());
    // 圧縮ペイロードの中程を壊し、CRC不一致にする。
    let flip_at = pkg.len() / 2;
    pkg[flip_at] ^= 0xff;
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("CRC不一致は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn gzipの途中切れは失敗する() {
    let pkg = package_bytes(&basic_members());
    let truncated = &pkg[..pkg.len() / 2];
    let error = unpack_fixture(truncated, &UnpackOptions::default())
        .2
        .expect_err("途中切れは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn tarの破損は失敗する() {
    let mut tar = tar_bytes(&basic_members());
    // 先頭ヘッダのチェックサムを壊す。
    tar[148] ^= 0xff;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    let pkg = encoder.finish().expect("gzipは完成する");
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("tar破損は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn tar終端後の非ゼロデータは失敗する() {
    let mut tar = tar_bytes(&basic_members());
    // 終端ゼロブロックの直後に非ゼロブロックを足す。
    tar.extend_from_slice(&[0xabu8; 512]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    let pkg = encoder.finish().expect("gzipは完成する");
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("終端後データは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn tarエントリ数の境界と超過を確認する() {
    // 基本パッケージは7エントリ（GUIDディレクトリ2件を含む）である。
    let count = 7u64;
    let pkg = package_bytes(&basic_members());
    let options = UnpackOptions::new().with_max_entries(count);
    unpack_fixture(&pkg, &options)
        .2
        .expect("上限ちょうどは成功する");
    let options = UnpackOptions::new().with_max_entries(count - 1);
    let error = unpack_fixture(&pkg, &options)
        .2
        .expect_err("上限超過は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::LimitExceeded);
}

#[test]
fn 総読み取り量の境界と超過を確認する() {
    let pkg = package_bytes(&basic_members());
    // 展開後の総読み取り量をgzip展開で測る。
    let mut decoder = GzDecoder::new(&pkg[..]);
    let mut total = Vec::new();
    decoder.read_to_end(&mut total).expect("展開は成功する");
    let exact = total.len() as u64;
    assert!(exact > 0);
    let options = UnpackOptions::new().with_max_total_bytes(exact);
    unpack_fixture(&pkg, &options)
        .2
        .expect("上限ちょうどは成功する");
    let options = UnpackOptions::new().with_max_total_bytes(exact - 1);
    let error = unpack_fixture(&pkg, &options)
        .2
        .expect_err("上限超過は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::LimitExceeded);
}

#[test]
fn pathname一件と合計の境界と超過を確認する() {
    // 一件の境界：`Assets/a.txt`(12バイト)は上限12で成功、11で失敗する。
    let pkg = package_bytes(&basic_members());
    let options = UnpackOptions::new().with_max_pathname_bytes(12);
    unpack_fixture(&pkg, &options)
        .2
        .expect("一件の上限ちょうどは成功する");
    let options = UnpackOptions::new().with_max_pathname_bytes(11);
    let error = unpack_fixture(&pkg, &options)
        .2
        .expect_err("一件の上限超過は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::LimitExceeded);
    // 合計の境界：2件の合計24バイトは上限23で失敗する。
    let options = UnpackOptions::new().with_max_pathname_total_bytes(23);
    unpack_fixture(&pkg, &options)
        .2
        .expect_err("合計の上限超過は失敗する");
}

#[test]
fn 既存の出力先は種類を問わず失敗する() {
    let pkg = package_bytes(&basic_members());
    let options = UnpackOptions::default();
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    // 既存ファイル。
    let file_dest = parent.path().join("file");
    fs::write(&file_dest, b"existing").expect("準備は成功する");
    let error = unpack_to(&pkg[..], &file_dest, &options).expect_err("既存ファイルは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
    // 既存の空ディレクトリ。
    let dir_dest = parent.path().join("dir");
    fs::create_dir(&dir_dest).expect("準備は成功する");
    let error = unpack_to(&pkg[..], &dir_dest, &options).expect_err("既存ディレクトリは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
    // ぶら下がりシンボリックリンク。
    #[cfg(unix)]
    {
        let link_dest = parent.path().join("link");
        std::os::unix::fs::symlink(parent.path().join("存在しない"), &link_dest)
            .expect("準備は成功する");
        let error = unpack_to(&pkg[..], &link_dest, &options).expect_err("ぶら下がりは失敗する");
        assert_eq!(error.kind(), UnpackErrorKind::Conflict);
    }
}

#[test]
fn 親が無い出力先は失敗し何も作らない() {
    let pkg = package_bytes(&basic_members());
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let destination = parent.path().join("存在しない親/out");
    let error =
        unpack_to(&pkg[..], &destination, &UnpackOptions::default()).expect_err("親無しは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Storage);
    // 全検証完了まで出力先を作らないため、親も出力先も残らない。
    assert!(!destination.exists());
    assert!(!parent.path().join("存在しない親").exists());
}

#[cfg(unix)]
#[test]
fn 読み取り専用の親では出力が始まらず残存報告しない() {
    let pkg = package_bytes(&basic_members());
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let readonly = parent.path().join("readonly");
    fs::create_dir(&readonly).expect("準備は成功する");
    // 権限は厳密に戻せるよう数値で扱う。`set_readonly(false)` は使わない。
    use std::os::unix::fs::PermissionsExt;
    let original = fs::metadata(&readonly)
        .expect("準備は成功する")
        .permissions()
        .mode();
    fs::set_permissions(&readonly, fs::Permissions::from_mode(0o555)).expect("準備は成功する");
    let destination = readonly.join("out");
    let error =
        unpack_to(&pkg[..], &destination, &UnpackOptions::default()).expect_err("作成は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Storage);
    assert!(!destination.exists());
    assert!(error.partial_output().is_none());
    // 後続テストのため権限を戻す。
    fs::set_permissions(&readonly, fs::Permissions::from_mode(original)).expect("後始末は成功する");
}

#[test]
fn 転送失敗と末尾欠けの分類を確認する() {
    // 転送中の失敗は `Input`、末尾欠けは `InvalidPackage` とする。
    struct FailingReader {
        failures: usize,
    }
    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            if self.failures > 0 {
                self.failures -= 1;
                return Ok(0);
            }
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "転送失敗（テスト用）",
            ))
        }
    }
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let error = unpack_to(
        FailingReader { failures: 0 },
        parent.path().join("out"),
        &UnpackOptions::default(),
    )
    .expect_err("転送失敗はエラーになる");
    assert_eq!(error.kind(), UnpackErrorKind::Input);

    struct EofReader;
    impl Read for EofReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "末尾欠け（テスト用）",
            ))
        }
    }
    let error = unpack_to(
        EofReader,
        parent.path().join("out"),
        &UnpackOptions::default(),
    )
    .expect_err("末尾欠けはエラーになる");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

/// 診断文面に生の制御文字が混ざっていないことを確かめる。
fn assert_no_raw_controls(label: &str, text: &str) {
    for c in text.chars() {
        assert!(!c.is_control(), "{label}に生の制御文字が混ざる");
    }
}

#[test]
fn 依存エラーの文面に制御文字があっても診断は可視エスケープする() {
    // tar由来のエラーがヘッダ名をそのまま含む場合の代理として、
    // 制御文字を含む読み取り失敗で一覧開始の経路を通す。
    // なお `source()` 連鎖は原因特定のため原文を保ち、表示時に逃がす。
    struct MaliciousReader;
    impl Read for MaliciousReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "偽装ヘッダ\n\x1b[2JINJECTED\0",
            ))
        }
    }
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let error = unpack_to(
        MaliciousReader,
        parent.path().join("out"),
        &UnpackOptions::default(),
    )
    .expect_err("読み取り失敗はエラーになる");
    assert_eq!(error.kind(), UnpackErrorKind::Input);
    // 表示用の `Display` には生の制御文字を出さない。
    let display = format!("{error}");
    assert_no_raw_controls("Display", &display);
    // 可視エスケープであり、除去ではない。
    assert!(display.contains("\\n"), "改行を可視化する");
    assert!(display.contains("\\x1b"), "ESCを可視化する");
    assert!(display.contains("\\0"), "NULを可視化する");
    // 原因連鎖は保ち、CLI側の表示時に逃がす。
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn 悪意あるヘッダ名でも診断に生の制御文字が出ない() {
    // 改行・ESC・方向制御を含むtar内パスで失敗させ、診断の各表示を確かめる。
    let evil_member = "bad\n\x1b[2J\u{202e}INJECTED";
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
        Member::Raw {
            path: format!("{}/{evil_member}", guid(2)).into_bytes(),
            typeflag: b'0',
            link: Vec::new(),
            content: b"x".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("未対応メンバーは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
    // 表示用の `Display` には生の制御文字を出さない。
    // この失敗は原因を持たないため、連鎖の表示はCLI側で確かめる。
    let display = format!("{error}");
    assert_no_raw_controls("Display", &display);
    assert!(display.contains("\\n"), "改行を可視化する");
    assert!(display.contains("\\x1b"), "ESCを可視化する");

    // NULを含む `pathname` 内容でも同様に可視化する。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a\0b.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("NULを含むパスは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPath);
    let display = format!("{error}");
    assert_no_raw_controls("Display", &display);
    assert!(display.contains("\\0"), "NULを可視化する");
}

#[test]
fn assetsそのものはフォルダとして出てメタも付く() {
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"root-meta".to_vec(),
        },
    ]);
    let (_parent, destination, result) = unpack_fixture(&pkg, &UnpackOptions::default());
    let report = result.expect("Assetsそのもののフォルダは成功する");
    assert_eq!(report.entries().len(), 1);
    assert_eq!(report.entries()[0].kind, EntryKind::Directory);
    assert!(destination.join("Assets").is_dir());
    assert_eq!(
        fs::read(destination.join("Assets.meta")).expect("metaは読める"),
        b"root-meta"
    );
}

#[test]
fn 先頭成分の大小文字混在は衝突する() {
    // `assets` と `ASSETS` は暗黙の親として同じ場所を指すため衝突とする。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"a".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"ASSETS/b.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"b".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("先頭成分の混在は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
}

#[test]
fn 暗黙の親の大小文字違いは衝突する() {
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/Sub/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"a".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/sub/b.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"b".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("暗黙の親の衝突は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::Conflict);
}

#[test]
fn メタとプレビューの重複は失敗する() {
    // `asset.meta` の重複。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("meta重複は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);

    // `preview.png` の重複。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
        Member::Preview {
            guid: guid(1),
            content: b"p1".to_vec(),
        },
        Member::Preview {
            guid: guid(1),
            content: b"p2".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("preview重複は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn gzip末尾トレーラの破損は失敗する() {
    let mut pkg = package_bytes(&basic_members());
    // gzipトレーラ（CRC32とISIZEの8バイト）の末尾を壊す。
    let last = pkg.len() - 1;
    pkg[last] ^= 0xff;
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("トレーラ破損は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
}

#[test]
fn 不可視文字を含むヘッダ名とpathnameの失敗表示は生のバイトを残さない() {
    // U+061C・U+2060・U+2028・U+2029 は `is_control()` が偽になるため、
    // 制御文字だけの検査では素通りして生のUTF-8が残る。回帰として残す。
    // 文字の含有検査はそのUTF-8バイト列の含有検査と等しい。
    let invisible = ['\u{61c}', '\u{2060}', '\u{2028}', '\u{2029}'];

    // tarヘッダ名に不可視文字が混ざる場合（未対応メンバー扱いになる）。
    let header = format!("{}/bad\u{61c}name\u{2060}", guid(9));
    let pkg = package_bytes(&[Member::Raw {
        path: header.into_bytes(),
        typeflag: b'0',
        link: Vec::new(),
        content: b"x".to_vec(),
    }]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("未対応メンバーは失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPackage);
    let shown = format!("{error}");
    for c in invisible {
        assert!(
            !shown.contains(c),
            "ヘッダ由来の表示に生の不可視文字が残らない"
        );
    }

    // `pathname` 内容に不可視文字が混ざる場合（先頭成分の検証で失敗する）。
    let pathname = "Assets\u{2028}dir\u{2029}/a.txt".to_string();
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: pathname.as_bytes().to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    let error = unpack_fixture(&pkg, &UnpackOptions::default())
        .2
        .expect_err("先頭成分の不一致は失敗する");
    assert_eq!(error.kind(), UnpackErrorKind::InvalidPath);
    let shown = format!("{error}");
    for c in invisible {
        assert!(
            !shown.contains(c),
            "pathname由来の表示に生の不可視文字が残らない"
        );
    }
    // 診断の素は保持し、表示だけが逃がす。
    assert_eq!(error.pathname(), Some(pathname.as_str()));
}

/// 自プロセスのRSS(KiB)を返す。macOSとLinuxの `ps` で動く。
#[cfg(unix)]
fn current_rss_kib() -> u64 {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .expect("psは実行できる");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("RSSは数値で取れる")
}

/// 実fixtureの全件内容一致と計測値を記録する。
///
/// `VRCTOM_UNITYPACKAGE_FIXTURE` に `.unitypackage` のパスを指定したときだけ
/// 実行し、未設定なら通常テストを壊さないよう跳ばす。RSS計測は `ps` を使う
/// ため Unix に限定する。
#[cfg(unix)]
#[test]
fn 実fixtureの全件内容一致と計測値を記録する() {
    let Some(path) = std::env::var_os("VRCTOM_UNITYPACKAGE_FIXTURE") else {
        eprintln!("VRCTOM_UNITYPACKAGE_FIXTURE 未設定のため実fixture計測を跳ばす");
        return;
    };
    let fixture = PathBuf::from(path);
    // 統合テストのカレントはcrateルート（`crates/vrctom`）のため、相対指定は
    // リポジトリルート基準に解決する。絶対パスはそのまま使う。
    // `CARGO_MANIFEST_DIR` は `crates/vrctom` を指すため、二つ上の親が基準になる。
    let fixture = if fixture.is_absolute() {
        fixture
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|parent| parent.parent())
            .expect("CARGO_MANIFEST_DIRに二つ上の親がある")
            .join(fixture)
    };
    let bytes = fs::read(&fixture).expect("実fixtureは読める");
    let started = std::time::Instant::now();
    let baseline = current_rss_kib();
    // 展開中の最大RSSを別スレッドで測る。
    let peak = Arc::new(AtomicU64::new(baseline));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let peak = Arc::clone(&peak);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let rss = current_rss_kib();
                peak.fetch_max(rss, Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        })
    };
    let parent = tempfile::TempDir::new().expect("親領域は作れる");
    let destination = parent.path().join("out");
    let report = unpack_to(&bytes[..], &destination, &UnpackOptions::default())
        .expect("実fixtureの展開は成功する");
    stop.store(true, Ordering::Relaxed);
    sampler.join().expect("計測スレッドは終わる");
    let elapsed = started.elapsed();
    let peak_rss = peak.load(Ordering::Relaxed);
    // 出力内容をtarと突き合わせて固定バッファで比べ、メモリに載せない。
    let mut archive = Archive::new(GzDecoder::new(&bytes[..]));
    let mut checked_files = 0usize;
    let mut spill_total = 0u64;
    let mut largest_asset = 0u64;
    let mut expected_paths = HashSet::new();
    for entry in archive.entries().expect("tarは読める") {
        let entry = entry.expect("エントリは読める");
        let path = entry.path_bytes().into_owned();
        let text = String::from_utf8_lossy(&path).into_owned();
        let Some((guid_raw, member)) = text.split_once('/') else {
            continue;
        };
        if member != "asset" && member != "asset.meta" {
            continue;
        }
        // GUIDは小文字に統一して照合する。
        let guid = guid_raw.to_ascii_lowercase();
        let pathname = report
            .entries()
            .iter()
            .find(|entry| entry.guid == guid)
            .unwrap_or_else(|| panic!("GUIDは報告にある: {guid}"))
            .pathname
            .clone();
        let rel = if member == "asset" {
            pathname.clone()
        } else {
            format!("{pathname}.meta")
        };
        expected_paths.insert(rel.clone());
        let mut expected = entry;
        let mut actual = File::open(destination.join(&rel)).expect("出力はある");
        spill_total += expected.size();
        if member == "asset" {
            largest_asset = largest_asset.max(expected.size());
        }
        // 内容は固定バッファで少しずつ比べ、メモリに載せない。
        // `read` は短い読みを返す場合があるため、両側の歩調は合わせない。
        let mut expected_chunk = [0u8; 65536];
        let mut actual_chunk = [0u8; 65536];
        loop {
            let expected_read = expected.read(&mut expected_chunk).expect("読める");
            if expected_read == 0 {
                assert_eq!(
                    actual.read(&mut actual_chunk).expect("読める"),
                    0,
                    "末尾が一致する: {rel}"
                );
                break;
            }
            let mut compared = 0;
            while compared < expected_read {
                let actual_read = actual
                    .read(&mut actual_chunk[compared..expected_read])
                    .expect("読める");
                assert_ne!(actual_read, 0, "長さが一致する: {rel}");
                compared += actual_read;
            }
            assert_eq!(
                &expected_chunk[..expected_read],
                &actual_chunk[..expected_read]
            );
        }
        checked_files += 1;
    }
    // 出力先の実ファイルを列挙し、期待集合と一致することを確かめる。
    // `preview.png` などの余分な出力が無いこともここで分かる。
    fn walk(dir: &Path, base: &Path, out: &mut HashSet<String>) {
        for entry in fs::read_dir(dir).expect("一覧は成功する") {
            let entry = entry.expect("項目は読める");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.insert(
                    path.strip_prefix(base)
                        .expect("配下である")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut actual_paths = HashSet::new();
    walk(&destination, &destination, &mut actual_paths);
    assert_eq!(actual_paths, expected_paths);
    assert_eq!(checked_files, expected_paths.len());
    assert_eq!(report.output_bytes(), spill_total);
    eprintln!(
        "実fixture計測: エントリ{}件（ファイル{}件 フォルダ{}件） 出力{}バイト 退避{}バイト 経過{:.1?} RSS基準{}KiB 最大{}KiB 最大asset{}バイト",
        report.entries().len(),
        report.file_count(),
        report.directory_count(),
        report.output_bytes(),
        spill_total,
        elapsed,
        baseline,
        peak_rss,
        largest_asset,
    );
    // 展開のRSS増加は固定バッファと管理情報だけに収まり、最大assetに比例しない。
    let growth_kib = peak_rss.saturating_sub(baseline);
    assert!(
        growth_kib * 1024 < largest_asset,
        "RSS増加{growth_kib}KiBは最大asset{largest_asset}バイトより小さい"
    );
}

/// 最小のフォルダmetaを組み立てる。GUIDはtar側と合わせる。
fn folder_meta(n: u128) -> Vec<u8> {
    format!(
        "fileFormatVersion: 2\nguid: {}\nfolderAsset: yes\nDefaultImporter:\n  externalObjects: {{}}\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n",
        guid(n)
    )
    .into_bytes()
}

/// 最小のファイルmetaを組み立てる。GUIDはtar側と合わせる。
fn file_meta(n: u128, importer: &str, body: &str) -> Vec<u8> {
    format!(
        "fileFormatVersion: 2\nguid: {}\n{importer}:\n{body}",
        guid(n)
    )
    .into_bytes()
}

#[test]
fn メモリ展開は出力展開と同じ一覧と内容になる() {
    // ファイル・空ファイル・フォルダ・読み捨て（preview）を含む配置で、
    // 退避先の違いが結果に影響しないことを確かめる。
    let members = vec![
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"hello".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"meta-a".to_vec(),
        },
        Member::Preview {
            guid: guid(1),
            content: b"preview-bytes".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/empty.bin".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: Vec::new(),
        },
        Member::Meta {
            guid: guid(2),
            content: Vec::new(),
        },
        Member::Dir { guid: guid(3) },
        Member::Pathname {
            guid: guid(3),
            content: b"Assets/Empty".to_vec(),
        },
        Member::Meta {
            guid: guid(3),
            content: b"meta-empty".to_vec(),
        },
    ];
    let pkg = package_bytes(&members);
    let options = UnpackOptions::default();
    let (_parent, destination, result) = unpack_fixture(&pkg, &options);
    let report = result.expect("出力展開は成功する");
    let memory = unpack_in_memory(&pkg[..], &options).expect("メモリ展開は成功する");
    // 一覧と集計が一致する。
    assert_eq!(memory.entries(), report.entries());
    assert_eq!(memory.file_count(), report.file_count());
    assert_eq!(memory.directory_count(), report.directory_count());
    assert_eq!(memory.output_bytes(), report.output_bytes());
    // 読み出し内容が出力先の実ファイルと一致する。
    for entry in report.entries() {
        if entry.kind == EntryKind::File {
            let expected = fs::read(destination.join(&entry.pathname)).expect("出力は読める");
            assert_eq!(
                memory.read(&entry.pathname),
                Some(expected.as_slice()),
                "実体の内容が一致する: {}",
                entry.pathname,
            );
        } else {
            // ディレクトリ自体に内容はない。
            assert_eq!(
                memory.read(&entry.pathname),
                None,
                "フォルダ自体は読めない: {}",
                entry.pathname,
            );
        }
        let meta_path = format!("{}.meta", entry.pathname);
        let expected = fs::read(destination.join(&meta_path)).expect("metaは読める");
        assert_eq!(
            memory.read(&meta_path),
            Some(expected.as_slice()),
            "metaの内容が一致する: {meta_path}",
        );
    }
    // 読み捨て（preview）や存在しないパスは読めない。
    assert_eq!(memory.read("Assets/preview.png"), None);
    assert_eq!(memory.read("Assets/ない.txt"), None);
}

#[test]
fn メモリ展開も壊れた入力を同じ分類で拒否する() {
    /// メモリ展開の失敗種別だけを取り出す。
    fn memory_kind(pkg: &[u8]) -> UnpackErrorKind {
        unpack_in_memory(pkg, &UnpackOptions::default())
            .expect_err("壊れた入力は失敗する")
            .kind()
    }

    // tar構造の破損。
    let mut tar = tar_bytes(&basic_members());
    tar[148] ^= 0xff;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    assert_eq!(
        memory_kind(&encoder.finish().expect("gzipは完成する")),
        UnpackErrorKind::InvalidPackage,
        "tar破損は壊れたパッケージになる",
    );

    // 同一GUIDの同一メンバー再出現。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"one".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"two".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPackage,
        "asset重複は壊れたパッケージになる",
    );

    // 必須欠損。
    let pkg = package_bytes(&[
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPackage,
        "pathname欠損は壊れたパッケージになる",
    );

    // 未対応のtar種別（symlink）。
    let pkg = package_bytes(&[Member::Raw {
        path: format!("{}/asset", guid(1)).into_bytes(),
        typeflag: b'2',
        link: b"Assets/target".to_vec(),
        content: b"x".to_vec(),
    }]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPackage,
        "symlinkは壊れたパッケージになる",
    );

    // 未対応のメンバー。
    let pkg = package_bytes(&[Member::Raw {
        path: format!("{}/unknown", guid(1)).into_bytes(),
        typeflag: b'0',
        link: Vec::new(),
        content: b"x".to_vec(),
    }]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPackage,
        "未対応メンバーは壊れたパッケージになる",
    );

    // 別GUIDの同一pathname。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/same.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"same".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"same".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/same.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"same".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"same".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::Conflict,
        "別GUIDの同一pathnameは衝突になる",
    );

    // ファイルとディレクトリの衝突。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"file".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/a/b.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"nested".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::Conflict,
        "親がファイルになる構造は衝突になる",
    );

    // 大小文字だけ異なる名前の混在。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/Foo.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"a".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m1".to_vec(),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/foo.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"b".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: b"m2".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::Conflict,
        "大小文字の衝突は衝突になる",
    );

    // パス脱出。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/../escape.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPath,
        "パス脱出は不正パスになる",
    );

    // Assets外。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Packages/foo.prefab".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: b"m".to_vec(),
        },
    ]);
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPath,
        "Assets外は不正パスになる",
    );

    // gzipのCRC不一致。
    let mut pkg = package_bytes(&basic_members());
    let flip_at = pkg.len() / 2;
    pkg[flip_at] ^= 0xff;
    assert_eq!(
        memory_kind(&pkg),
        UnpackErrorKind::InvalidPackage,
        "CRC不一致は壊れたパッケージになる",
    );

    // gzipの途中切れ。
    let pkg = package_bytes(&basic_members());
    assert_eq!(
        memory_kind(&pkg[..pkg.len() / 2]),
        UnpackErrorKind::InvalidPackage,
        "途中切れは壊れたパッケージになる",
    );

    // tar終端後の非ゼロデータ。
    let mut tar = tar_bytes(&basic_members());
    tar.extend_from_slice(&[0xabu8; 512]);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar).expect("gzip書き込みは成功する");
    assert_eq!(
        memory_kind(&encoder.finish().expect("gzipは完成する")),
        UnpackErrorKind::InvalidPackage,
        "終端後データは壊れたパッケージになる",
    );
}

#[test]
fn メモリ展開も資源上限を同じ意味で適用する() {
    let pkg = package_bytes(&basic_members());

    // tarエントリ数は7件（GUIDディレクトリ2件を含む）であり、
    // 上限ちょうどは成功し、1件不足では失敗する。
    let options = UnpackOptions::new().with_max_entries(7);
    unpack_in_memory(&pkg[..], &options).expect("上限ちょうどは成功する");
    let options = UnpackOptions::new().with_max_entries(6);
    assert_eq!(
        unpack_in_memory(&pkg[..], &options)
            .expect_err("上限超過は失敗する")
            .kind(),
        UnpackErrorKind::LimitExceeded,
        "エントリ数超過は上限超過になる",
    );

    // 総読み取り量も上限ちょうどは成功し、1バイト不足では失敗する。
    let mut decoder = GzDecoder::new(&pkg[..]);
    let mut total = Vec::new();
    decoder.read_to_end(&mut total).expect("展開は成功する");
    let exact = total.len() as u64;
    let options = UnpackOptions::new().with_max_total_bytes(exact);
    unpack_in_memory(&pkg[..], &options).expect("上限ちょうどは成功する");
    let options = UnpackOptions::new().with_max_total_bytes(exact - 1);
    assert_eq!(
        unpack_in_memory(&pkg[..], &options)
            .expect_err("上限超過は失敗する")
            .kind(),
        UnpackErrorKind::LimitExceeded,
        "総量超過は上限超過になる",
    );

    // `pathname` 一件と合計の上限も同じ意味で効く。
    let options = UnpackOptions::new().with_max_pathname_bytes(12);
    unpack_in_memory(&pkg[..], &options).expect("一件の上限ちょうどは成功する");
    let options = UnpackOptions::new().with_max_pathname_bytes(11);
    assert_eq!(
        unpack_in_memory(&pkg[..], &options)
            .expect_err("上限超過は失敗する")
            .kind(),
        UnpackErrorKind::LimitExceeded,
        "一件超過は上限超過になる",
    );
    let options = UnpackOptions::new().with_max_pathname_total_bytes(23);
    assert_eq!(
        unpack_in_memory(&pkg[..], &options)
            .expect_err("上限超過は失敗する")
            .kind(),
        UnpackErrorKind::LimitExceeded,
        "合計超過は上限超過になる",
    );
}

#[test]
fn メモリの索引は基準相対パスで走査と同じ解決になる() {
    // モデル・材質・テクスチャ・フォルダ・Assets自体を含む配置で、
    // 種別分類と所在解決が走査と一致することを確かめる。
    let members = vec![
        Member::Dir { guid: guid(1) },
        Member::Pathname {
            guid: guid(1),
            content: b"Assets".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: folder_meta(1),
        },
        Member::Pathname {
            guid: guid(2),
            content: b"Assets/model.fbx".to_vec(),
        },
        Member::Asset {
            guid: guid(2),
            content: b"fake-fbx".to_vec(),
        },
        Member::Meta {
            guid: guid(2),
            content: file_meta(2, "ModelImporter", "  externalObjects: {}\n"),
        },
        Member::Pathname {
            guid: guid(3),
            content: b"Assets/Mats/hero.mat".to_vec(),
        },
        Member::Asset {
            guid: guid(3),
            content: b"fake-mat".to_vec(),
        },
        Member::Meta {
            guid: guid(3),
            content: file_meta(3, "NativeFormatImporter", "  externalObjects: {}\n"),
        },
        Member::Pathname {
            guid: guid(4),
            content: b"Assets/Textures/tex.png".to_vec(),
        },
        Member::Asset {
            guid: guid(4),
            content: b"\x89PNG-fake".to_vec(),
        },
        Member::Meta {
            guid: guid(4),
            content: file_meta(
                4,
                "TextureImporter",
                "  externalObjects: {}\n  textureType: 0\n",
            ),
        },
        Member::Dir { guid: guid(5) },
        Member::Pathname {
            guid: guid(5),
            content: b"Assets/Mats".to_vec(),
        },
        Member::Meta {
            guid: guid(5),
            content: folder_meta(5),
        },
    ];
    let pkg = package_bytes(&members);
    let memory = unpack_in_memory(&pkg[..], &UnpackOptions::default()).expect("展開は成功する");
    let index = memory
        .asset_index(&IndexOptions::new())
        .expect("索引は成功する");
    // `Assets` 自体は索引のルートであり、件にしない。
    let paths: Vec<&str> = index
        .entries()
        .iter()
        .map(|record| record.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["Mats", "Mats/hero.mat", "Textures/tex.png", "model.fbx"],
    );
    let kinds: Vec<AssetKind> = index.entries().iter().map(|record| record.kind).collect();
    assert_eq!(
        kinds,
        vec![
            AssetKind::Folder,
            AssetKind::Material,
            AssetKind::Texture,
            AssetKind::Model,
        ],
    );
    for (record, n) in index.entries().iter().zip([5u128, 3, 4, 2]) {
        assert_eq!(record.guid.as_str(), guid(n).as_str(), "GUIDが一致する");
    }
    // 実体とmetaの読み出しもできる。
    assert_eq!(
        memory.read("Assets/model.fbx"),
        Some(b"fake-fbx".as_slice())
    );
    assert_eq!(
        memory.read("Assets/model.fbx.meta"),
        Some(file_meta(2, "ModelImporter", "  externalObjects: {}\n").as_slice()),
    );

    // 同一ツリーを出力展開して `Assets` 下を走査した場合と同じ解決になる。
    let (_parent, destination, result) = unpack_fixture(&pkg, &UnpackOptions::default());
    result.expect("出力展開は成功する");
    let scanned = vrctom::unity::scan_directory(&destination.join("Assets"), &IndexOptions::new())
        .expect("走査は成功する");
    assert_eq!(scanned.entries().len(), index.entries().len());
    for (scanned, record) in scanned.entries().iter().zip(index.entries().iter()) {
        assert_eq!(scanned.path, record.path, "パスが一致する");
        assert_eq!(scanned.guid, record.guid, "GUIDが一致する");
        assert_eq!(scanned.kind, record.kind, "種別が一致する");
        // パス引きとGUID引きのどちらでも同じ件に届く。
        assert_eq!(
            index.get_by_path(&scanned.path).expect("ある").guid,
            record.guid,
        );
        assert_eq!(index.get(&scanned.guid).expect("ある").path, record.path);
    }
}

#[test]
fn メモリの索引は実体とmetaの種別不一致を拒否する() {
    // 実体（asset）があるのにフォルダmetaを持つ配置は、展開自体は通る。
    // metaの意味を見ないためである。索引では種別不一致として失敗する。
    let pkg = package_bytes(&[
        Member::Pathname {
            guid: guid(1),
            content: b"Assets/a.txt".to_vec(),
        },
        Member::Asset {
            guid: guid(1),
            content: b"x".to_vec(),
        },
        Member::Meta {
            guid: guid(1),
            content: folder_meta(1),
        },
    ]);
    let memory = unpack_in_memory(&pkg[..], &UnpackOptions::default()).expect("展開は成功する");
    let error = memory
        .asset_index(&IndexOptions::new())
        .expect_err("種別不一致は失敗する");
    assert_eq!(error.kind(), UnityErrorKind::InvalidMetadata);
    assert_eq!(error.path(), Some("a.txt"));
}
