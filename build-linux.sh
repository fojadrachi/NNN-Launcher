#!/usr/bin/env bash
# Baut die Linux-Version des NNN Launchers (in WSL oder unter Linux):
#   dist/NNN-Launcher-<Version>-linux-x64.tar.gz
# Enthält den Ordner NNN-Launcher/ mit dem Programm nnn-launcher. Beim ersten Start legt es
# einen Startmenü-Eintrag an und registriert nnnlauncher://-Links.
#
# Gebaut wird in ~/nnn-build (Linux-Dateisystem, viel schneller als /mnt). Für fontconfig reicht
# die installierte Laufzeitbibliothek: ein kleiner pkg-config-Ersatz meldet sie dem Build.
# Mit ZIG=1 wird gegen glibc 2.31 gebaut (läuft dann auch auf älteren Distributionen),
# dafür werden zig und cargo-zigbuild in ~/nnn-build gebraucht.
set -euo pipefail

SRC="$(cd "$(dirname "$0")" && pwd)"
WORK="$HOME/nnn-build"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$SRC/Cargo.toml" | head -1)"
echo "NNN Launcher $VERSION für Linux"

mkdir -p "$WORK/tools" "$WORK/fclib"
ln -sf /usr/lib/x86_64-linux-gnu/libfontconfig.so.1 "$WORK/fclib/libfontconfig.so"
cat > "$WORK/tools/pkg-config" <<EOF
#!/bin/sh
# pkg-config-Ersatz: kennt nur fontconfig
case "\$*" in *fontconfig*) ;; *) exit 1 ;; esac
case "\$*" in *--modversion*) echo 2.17.1; exit 0 ;; esac
case "\$*" in *--exists*|*--atleast-version*) exit 0 ;; esac
echo "-L$WORK/fclib -lfontconfig"
EOF
chmod +x "$WORK/tools/pkg-config"

rsync -a --delete --exclude target --exclude dist --exclude .git "$SRC/" "$WORK/src-copy/"
cd "$WORK/src-copy"
export PKG_CONFIG="$WORK/tools/pkg-config"
if [ "${ZIG:-0}" = "1" ]; then
  export PATH="$WORK/zig:$PATH"
  cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.31
  BIN="target/x86_64-unknown-linux-gnu/release/nnn-launcher"
else
  cargo build --release
  BIN="target/release/nnn-launcher"
fi

STAGE="$WORK/stage/NNN-Launcher"
rm -rf "$WORK/stage" && mkdir -p "$STAGE"
cp "$BIN" "$STAGE/nnn-launcher"
cp assets/icon.png "$STAGE/nnn-launcher.png"
cat > "$STAGE/LIESMICH.txt" <<'EOF'
NNN Launcher für Linux

Starten:  ./nnn-launcher
Beim ersten Start erscheint der Launcher im Startmenü, Einladungslinks (nnnlauncher://) öffnen ihn direkt.
Daten liegen in ~/.local/share/NNN Launcher.

Benötigt wird eine grafische Oberfläche (X11 oder Wayland) mit fontconfig und libxkbcommon,
das ist auf üblichen Desktop-Systemen vorhanden. Gespielt wird über den offiziellen Minecraft
Launcher; fehlt er, lädt der NNN Launcher ihn einmalig von Mojang.
EOF
mkdir -p "$SRC/dist"
OUT="$SRC/dist/NNN-Launcher-$VERSION-linux-x64.tar.gz"
tar -czf "$OUT" -C "$WORK/stage" NNN-Launcher
ls -la "$OUT"
objdump -T "$STAGE/nnn-launcher" | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1 | sed 's/^/benötigt mindestens /'
