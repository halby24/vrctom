//! 一時作業領域の契約テスト（公開 API のみを使う）。

#![cfg(not(target_arch = "wasm32"))]

use futures::executor::block_on;
use vrctom::workspace::{EntryKind, Workspace, WorkspaceErrorKind};

/// テスト名は日本語で書く（テストは仕様・契約であるため）。

#[test]
fn 新規領域のルート一覧は空である() {
    let mut workspace = Workspace::new();
    let entries = block_on(workspace.list("")).expect("ルート一覧は成功する");
    assert!(entries.is_empty());
}

#[test]
fn 異なる領域は内容と削除が干渉しない() {
    let mut first = Workspace::new();
    let mut second = Workspace::new();
    block_on(first.write("shared.txt", b"first")).expect("書き込みは成功する");
    block_on(second.write("shared.txt", b"second")).expect("書き込みは成功する");
    assert_eq!(
        block_on(first.read("shared.txt")).expect("読み取りは成功する"),
        b"first"
    );
    assert_eq!(
        block_on(second.read("shared.txt")).expect("読み取りは成功する"),
        b"second"
    );
    block_on(first.delete("shared.txt")).expect("削除は成功する");
    assert_eq!(
        block_on(first.list("")).expect("一覧は成功する"),
        Vec::new()
    );
    assert_eq!(
        block_on(second.read("shared.txt")).expect("他方の内容は残る"),
        b"second"
    );
}

#[test]
fn バイナリと空データの読み書きが一致する() {
    let mut workspace = Workspace::new();
    let binary: Vec<u8> = (0u8..=255u8).collect();
    block_on(workspace.write("binary.dat", &binary)).expect("書き込みは成功する");
    assert_eq!(
        block_on(workspace.read("binary.dat")).expect("読み取りは成功する"),
        binary
    );
    block_on(workspace.write("empty.dat", &[])).expect("空データの書き込みは成功する");
    assert_eq!(
        block_on(workspace.read("empty.dat")).expect("読み取りは成功する"),
        Vec::<u8>::new()
    );
}

#[test]
fn 短い内容で置換すると末尾が残らない() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("a.txt", b"0123456789")).expect("書き込みは成功する");
    block_on(workspace.write("a.txt", b"ab")).expect("置換は成功する");
    assert_eq!(
        block_on(workspace.read("a.txt")).expect("読み取りは成功する"),
        b"ab"
    );
}

#[test]
fn ネストした書き込みで親ディレクトリが作られる() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("a/b/c.txt", b"nested")).expect("書き込みは成功する");
    assert_eq!(
        block_on(workspace.read("a/b/c.txt")).expect("読み取りは成功する"),
        b"nested"
    );
    let entries = block_on(workspace.list("")).expect("一覧は成功する");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "a");
    assert_eq!(entries[0].kind, EntryKind::Directory);
}

#[test]
fn 一覧は直下のみを種別と昇順で返す() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("b.txt", b"b")).expect("書き込みは成功する");
    block_on(workspace.write("a.txt", b"a")).expect("書き込みは成功する");
    block_on(workspace.write("dir/nested.txt", b"n")).expect("書き込みは成功する");
    block_on(workspace.write("dir/sub/deep.txt", b"d")).expect("書き込みは成功する");

    let entries = block_on(workspace.list("")).expect("ルート一覧は成功する");
    let paths: Vec<(&str, EntryKind)> = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect();
    // 直下のみを相対パスの UTF-8 バイト列で昇順に返す。
    assert_eq!(
        paths,
        vec![
            ("a.txt", EntryKind::File),
            ("b.txt", EntryKind::File),
            ("dir", EntryKind::Directory),
        ]
    );

    let entries = block_on(workspace.list("dir")).expect("下位一覧は成功する");
    let paths: Vec<(&str, EntryKind)> = entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry.kind))
        .collect();
    assert_eq!(
        paths,
        vec![
            ("dir/nested.txt", EntryKind::File),
            ("dir/sub", EntryKind::Directory),
        ]
    );
}

#[test]
fn 存在しない読み取りと一覧は見つからない扱いになる() {
    let mut workspace = Workspace::new();
    let error = block_on(workspace.read("missing.txt")).expect_err("存在しない読み取りは失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::NotFound);
    let error = block_on(workspace.list("missing")).expect_err("存在しない一覧は失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::NotFound);
    // 途中の親が存在しない場合も存在しない扱いにする。
    let error =
        block_on(workspace.read("missing/inner.txt")).expect_err("親がない読み取りは失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::NotFound);
}

#[test]
fn 種別違いの読み取りと一覧は種別違い扱いになる() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("file.txt", b"x")).expect("書き込みは成功する");
    block_on(workspace.write("dir/inner.txt", b"y")).expect("書き込みは成功する");

    let error = block_on(workspace.read("dir")).expect_err("ディレクトリの読み取りは失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::WrongEntryType);
    let error = block_on(workspace.list("file.txt")).expect_err("ファイルの一覧は失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::WrongEntryType);
    let error = block_on(workspace.read("file.txt/inner.txt"))
        .expect_err("ファイル配下の読み取りは失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::WrongEntryType);
}

#[test]
fn 削除はファイルと再帰に対応し存在しなくても成功する() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("gone.txt", b"x")).expect("書き込みは成功する");
    block_on(workspace.delete("gone.txt")).expect("ファイル削除は成功する");
    let error = block_on(workspace.read("gone.txt")).expect_err("削除後は読めない");
    assert_eq!(error.kind(), WorkspaceErrorKind::NotFound);

    block_on(workspace.write("tree/keep.txt", b"k")).expect("書き込みは成功する");
    block_on(workspace.write("tree/sub/deep.txt", b"d")).expect("書き込みは成功する");
    block_on(workspace.write("sibling.txt", b"s")).expect("書き込みは成功する");
    block_on(workspace.delete("tree/sub")).expect("再帰削除は成功する");
    // 子を消しても親ディレクトリは残り、兄弟は保持される。
    let entries = block_on(workspace.list("tree")).expect("親は残る");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "tree/keep.txt");
    assert_eq!(entries[0].kind, EntryKind::File);
    assert_eq!(
        block_on(workspace.read("sibling.txt")).expect("兄弟は残る"),
        b"s"
    );

    block_on(workspace.delete("missing.txt")).expect("存在しない削除は成功する");
    block_on(workspace.delete("missing/sub/tree.txt")).expect("存在しない親配下の削除は成功する");
}

#[test]
fn 不正なパスは全操作で拒否され内容が変わらない() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("ok.txt", b"ok")).expect("書き込みは成功する");

    // 空文字列は `list` のルート指定以外では不正とする。
    let invalid_paths = [
        "",
        "/",
        "/absolute.txt",
        "./relative.txt",
        "../escape.txt",
        "a/./b.txt",
        "a/../b.txt",
        "a//b.txt",
        "trailing/",
        "back\\slash.txt",
        "C:/drive.txt",
        "C:drive.txt",
        "colon:name.txt",
        "less<than.txt",
        "greater>than.txt",
        "quote\"d.txt",
        "pipe|d.txt",
        "question?.txt",
        "star*.txt",
        "CON",
        "con",
        "aux.txt",
        "COM1",
        "lpt9.dat",
        "NUL",
        "COM¹",
        "com².txt",
        "LPT³/log",
        "lpt¹",
        "Com³.dat",
        "trailing-space /x.txt",
        "trailing-dot./x.txt",
        "x.txt ",
        "x.txt.",
        "nul-char\0.txt",
        "control\x1Fchar.txt",
        "del\x7Fchar.txt",
    ];
    for path in invalid_paths {
        let error =
            block_on(workspace.read(path)).expect_err(&format!("読み取りを拒否する: {path:?}"));
        assert_eq!(
            error.kind(),
            WorkspaceErrorKind::InvalidPath,
            "path: {path:?}"
        );
        let error = block_on(workspace.write(path, b"x"))
            .expect_err(&format!("書き込みを拒否する: {path:?}"));
        assert_eq!(
            error.kind(),
            WorkspaceErrorKind::InvalidPath,
            "path: {path:?}"
        );
        let error =
            block_on(workspace.delete(path)).expect_err(&format!("削除を拒否する: {path:?}"));
        assert_eq!(
            error.kind(),
            WorkspaceErrorKind::InvalidPath,
            "path: {path:?}"
        );
        // 不正パスではストレージ操作を開始しないため、既存内容は変わらない。
        let entries = block_on(workspace.list("")).expect("一覧は成功する");
        assert_eq!(entries.len(), 1, "path: {path:?}");
        assert_eq!(entries[0].path, "ok.txt", "path: {path:?}");
    }
    // `list` の不正パスも拒否する（空文字列だけはルート指定として有効）。
    for path in invalid_paths.into_iter().filter(|path| !path.is_empty()) {
        let error = block_on(workspace.list(path)).expect_err(&format!("一覧を拒否する: {path:?}"));
        assert_eq!(
            error.kind(),
            WorkspaceErrorKind::InvalidPath,
            "path: {path:?}"
        );
    }
    assert_eq!(
        block_on(workspace.read("ok.txt")).expect("既存内容は変わらない"),
        b"ok"
    );
}

#[test]
fn ルートに対する読み書き削除は拒否され一覧だけが成功する() {
    let mut workspace = Workspace::new();
    let error = block_on(workspace.read("")).expect_err("ルートの読み取りは失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
    let error = block_on(workspace.write("", b"x")).expect_err("ルートの書き込みは失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
    let error = block_on(workspace.delete("")).expect_err("ルートの削除は失敗する");
    assert_eq!(error.kind(), WorkspaceErrorKind::InvalidPath);
    let entries = block_on(workspace.list("")).expect("ルートの一覧は成功する");
    assert!(entries.is_empty());
}

#[test]
fn 未使用の終了と使用後の終了と二重終了が成功する() {
    let mut unused = Workspace::new();
    block_on(unused.close()).expect("未使用の終了は成功する");
    block_on(unused.close()).expect("二重終了は成功する");

    let mut used = Workspace::new();
    block_on(used.write("a.txt", b"a")).expect("書き込みは成功する");
    block_on(used.close()).expect("使用後の終了は成功する");
    block_on(used.close()).expect("二重終了は成功する");
}

#[test]
fn 終了後の通常操作は終了済み扱いで拒否される() {
    let mut workspace = Workspace::new();
    block_on(workspace.write("a.txt", b"a")).expect("書き込みは成功する");
    block_on(workspace.close()).expect("終了は成功する");
    for error in [
        block_on(workspace.read("a.txt")).expect_err("終了後の読み取りは失敗する"),
        block_on(workspace.write("b.txt", b"b")).expect_err("終了後の書き込みは失敗する"),
        block_on(workspace.delete("a.txt")).expect_err("終了後の削除は失敗する"),
        block_on(workspace.list("")).expect_err("終了後の一覧は失敗する"),
    ] {
        assert_eq!(error.kind(), WorkspaceErrorKind::Closed);
    }
}

#[test]
fn 一方の終了は他方に影響しない() {
    let mut first = Workspace::new();
    let mut second = Workspace::new();
    block_on(first.write("a.txt", b"first")).expect("書き込みは成功する");
    block_on(second.write("a.txt", b"second")).expect("書き込みは成功する");
    block_on(first.close()).expect("一方の終了は成功する");
    assert_eq!(
        block_on(second.read("a.txt")).expect("他方は使える"),
        b"second"
    );
    block_on(second.close()).expect("他方の終了は成功する");
}

#[test]
fn 操作や終了の予約を取り消しても終了できる() {
    let mut workspace = Workspace::new();
    // poll せずに取り消した操作は何も開始しない。
    drop(workspace.write("a.txt", b"data"));
    drop(workspace.read("a.txt"));
    drop(workspace.delete("a.txt"));
    drop(workspace.list(""));
    drop(workspace.close());
    // 終了前に一覧で確かめる。取り消した書き込みは反映されていない。
    let entries = block_on(workspace.list("")).expect("終了前の一覧は成功する");
    assert!(entries.is_empty());
    block_on(workspace.close()).expect("終了は成功する");
    // 終了済みのため `Closed` で拒否されることで、後続操作の可否だけを確かめる。
    let error = block_on(workspace.read("a.txt")).expect_err("終了後は操作できない");
    assert_eq!(error.kind(), WorkspaceErrorKind::Closed);
}
