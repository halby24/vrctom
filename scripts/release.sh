#!/bin/sh
# workspace の版を上げ、コミット・タグ・push まで行う。
# npm への公開はタグの push を受けて .github/workflows/npm.yml が行う。
#
# 使い方（cargo などを mise から使うため mise 経由で実行する）:
#   mise run release patch|minor|major|X.Y.Z [--dry-run]
#
# --dry-run は前提の確認と次の版の表示だけを行い、何も変更しない。
set -eu

usage() {
  echo "使い方: mise run release patch|minor|major|X.Y.Z [--dry-run]" >&2
  exit 2
}

bump=""
dry_run=false
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=true ;;
    -*) usage ;;
    *)
      [ -z "$bump" ] || usage
      bump=$arg
      ;;
  esac
done
[ -n "$bump" ] || usage

fail() {
  echo "release: $*" >&2
  exit 1
}

cd "$(git rev-parse --show-toplevel)"

# [workspace.package] の version を読む。
current=$(awk '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && /^version *= *"/ { gsub(/.*= *"|".*/, ""); print; exit }
' Cargo.toml)
echo "$current" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
  || fail "Cargo.toml の workspace 版を読めない: '$current'"

major=${current%%.*}
rest=${current#*.}
minor=${rest%%.*}
patch=${rest#*.}
case "$bump" in
  patch) next="$major.$minor.$((patch + 1))" ;;
  minor) next="$major.$((minor + 1)).0" ;;
  major) next="$((major + 1)).0.0" ;;
  *)
    echo "$bump" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || usage
    next=$bump
    ;;
esac
tag="v$next"

# 前提を確かめる。
branch=$(git symbolic-ref --short HEAD 2>/dev/null || true)
[ "$branch" = "main" ] || fail "main ブランチで実行すること（現在: ${branch:-detached}）"
[ -z "$(git status --porcelain)" ] || fail "作業ツリーに未コミットの変更がある"
git fetch --quiet --tags origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] \
  || fail "main が origin/main と一致しない。pull か push を先に行うこと"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && fail "タグ $tag は既にある"

prev_tag="v$current"
if git rev-parse -q --verify "refs/tags/$prev_tag" >/dev/null; then
  range="$prev_tag..HEAD"
else
  range="HEAD"
fi
changes=$(git log --format='- %s' "$range")
[ -n "$changes" ] || fail "$prev_tag 以降のコミットがない"

echo "版: $current -> $next"
echo "含まれる変更:"
echo "$changes"

if [ "$dry_run" = true ]; then
  echo "（dry-run のため変更しない）"
  exit 0
fi

mise run check
mise run test

# [workspace.package] の version だけを書き換える。
tmp=$(mktemp)
awk -v ver="$next" '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && !done && /^version *= *"/ { print "version = \"" ver "\""; done = 1; next }
  { print }
' Cargo.toml >"$tmp"
mv "$tmp" Cargo.toml
cargo update --workspace --quiet

git add Cargo.toml Cargo.lock
git commit --quiet -m "${next}へ版を上げる"
git tag -a "$tag" -m "$tag"
git push --quiet --atomic origin main "$tag"

echo "$tag を push した。npm への公開は GitHub Actions の npm ワークフローで進む。"
