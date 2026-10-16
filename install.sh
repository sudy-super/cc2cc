#!/bin/sh
set -eu

fail() {
    printf 'cc2cc: %s\n' "$*" >&2
    exit 1
}

check_skill() {
    target=$1
    [ ! -L "$target" ] || fail "スキルの配置先がシンボリックリンクです: $target"
    if [ -e "$target" ]; then
        [ -d "$target" ] && [ -f "$target/.cc2cc-managed" ] &&
            [ "$(cat "$target/.cc2cc-managed")" = 'sudy-super/cc2cc' ] ||
            fail "同名のスキルを上書きできません: $target"
        [ ! -L "$target/SKILL.md" ] || fail "SKILL.mdがシンボリックリンクです"
    fi
}

put_skill() {
    target=$1
    mkdir -p "$target"
    cp "$work/extracted/skills/cc2cc/SKILL.md" "$target/.SKILL.md.$$"
    chmod 644 "$target/.SKILL.md.$$"
    mv -f "$target/.SKILL.md.$$" "$target/SKILL.md"
    printf '%s\n' 'sudy-super/cc2cc' > "$target/.cc2cc-managed"
}

main() {
    version=${CC2CC_VERSION:-}
    if [ "$#" -gt 0 ]; then
        [ "$#" -eq 2 ] && [ "$1" = '--version' ] || fail '使い方: sh install.sh [--version vX.Y.Z]'
        version=$2
    fi
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64) platform=aarch64-apple-darwin ;;
        Linux/x86_64) platform=x86_64-unknown-linux-musl ;;
        *) fail 'Apple Silicon Macとx86_64 Linuxに対応しています' ;;
    esac
    bin_dir=${CC2CC_INSTALL_DIR:-"$HOME/.local/bin"}
    claude_skill=${CLAUDE_CONFIG_DIR:-"$HOME/.claude"}/skills/cc2cc
    claw_skill=${CLAWCODEX_CONFIG_DIR:-"$HOME/.clawcodex"}/skills/cc2cc
    for path in "$bin_dir" "$claude_skill" "$claw_skill"; do
        case "$path" in /*) ;; *) fail '配置先には絶対パスが必要です' ;; esac
    done
    [ ! -L "$bin_dir/cc2cc" ] || fail '本体の配置先がシンボリックリンクです'
    [ ! -d "$bin_dir/cc2cc" ] || fail '本体の配置先にディレクトリがあります'
    check_skill "$claude_skill"
    check_skill "$claw_skill"
    base=${CC2CC_RELEASE_BASE_URL:-https://github.com/sudy-super/cc2cc/releases}
    if [ -z "$version" ]; then
        latest=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$base/latest")
        version=${latest##*/}
    fi
    case "$version" in v[0-9]*) ;; *) fail 'リリースのバージョンが不正です' ;; esac
    case "$version" in *[!a-zA-Z0-9._-]*) fail 'リリースのバージョンが不正です' ;; esac
    umask 077
    work=$(mktemp -d "${TMPDIR:-/tmp}/cc2cc-install.XXXXXXXX")
    trap 'rm -rf "$work"' EXIT HUP INT TERM
    asset=cc2cc-$platform.tar.gz
    curl -fsSL "$base/download/$version/SHA256SUMS" -o "$work/SHA256SUMS"
    curl -fsSL "$base/download/$version/$asset" -o "$work/$asset"
    expected=$(awk -v name="$asset" '$2 == name { print $1; count++ } END { if (count != 1) exit 1 }' "$work/SHA256SUMS") || fail 'チェックサムがありません'
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$work/$asset")
    else
        command -v shasum >/dev/null 2>&1 || fail 'SHA-256を計算するコマンドがありません'
        actual=$(shasum -a 256 "$work/$asset")
    fi
    [ "${actual%% *}" = "$expected" ] || fail 'ダウンロードしたファイルのSHA-256が一致しません'
    tar -tzf "$work/$asset" > "$work/entries"
    while IFS= read -r entry; do
        case "$entry" in cc2cc|LICENSE|THIRD_PARTY_NOTICES.md|skills/|skills/cc2cc/|skills/cc2cc/SKILL.md) ;;
            *) fail "配布ファイルに想定外のパスがあります: $entry" ;;
        esac
    done < "$work/entries"
    tar -tvzf "$work/$asset" | awk 'substr($0,1,1) != "-" && substr($0,1,1) != "d" { exit 1 }' || fail '配布ファイルにリンクがあります'
    mkdir "$work/extracted"
    tar -xzf "$work/$asset" -C "$work/extracted"
    [ -f "$work/extracted/cc2cc" ] && [ -f "$work/extracted/skills/cc2cc/SKILL.md" ] || fail '配布ファイルが不足しています'
    chmod 755 "$work/extracted/cc2cc"
    "$work/extracted/cc2cc" --version
    mkdir -p "$bin_dir"
    stage=$(mktemp "$bin_dir/.cc2cc.XXXXXXXX")
    cp "$work/extracted/cc2cc" "$stage"
    chmod 755 "$stage"
    put_skill "$claude_skill"
    put_skill "$claw_skill"
    mv -f "$stage" "$bin_dir/cc2cc"
    printf 'インストールしました: %s\n' "$bin_dir/cc2cc"
    printf 'スキル: %s\nスキル: %s\n' "$claude_skill" "$claw_skill"
    case ":$PATH:" in *":$bin_dir:"*) ;; *) printf 'PATHに追加してください: %s\n' "$bin_dir" ;; esac
}

main "$@"
