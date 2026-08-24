#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
prefix=${PREFIX:-}
if [ -z "$prefix" ]; then
    if [ "$(uname -s)" = Darwin ]; then
        prefix="$HOME/.local"
    else
        prefix="$HOME/.local"
    fi
fi

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$prefix/bin/orc"
    if [ "$(uname -s)" = Darwin ]; then
        rm -rf "$HOME/Applications/Orc.app"
    else
        rm -f "$prefix/lib/orc/orc-desktop"
        rm -f "$prefix/share/applications/orc.desktop"
        rm -f "$prefix/share/icons/hicolor/128x128/apps/orc.png"
    fi
    exit 0
fi

cd "$project_dir"
npm ci
npm run typecheck
npm run build
cargo build --release --bin orc
npm run tauri:build
npm run validate:package
install -d "$prefix/bin"
install -m 0755 target/release/orc "$prefix/bin/orc"

if [ "$(uname -s)" = Darwin ]; then
    install -d "$HOME/Applications"
    rm -rf "$HOME/Applications/Orc.app"
    cp -R src-tauri/target/release/bundle/macos/Orc.app "$HOME/Applications/Orc.app"
else
    install -d "$prefix/lib/orc"
    install -m 0755 src-tauri/target/release/orc-desktop "$prefix/lib/orc/orc-desktop"
    install -d "$prefix/share/applications"
    install -m 0644 packaging/linux/orc.desktop "$prefix/share/applications/orc.desktop"
    install -d "$prefix/share/icons/hicolor/128x128/apps"
    install -m 0644 src-tauri/icons/128x128.png "$prefix/share/icons/hicolor/128x128/apps/orc.png"
fi
