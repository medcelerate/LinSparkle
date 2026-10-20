#!/usr/bin/env bash
# Build LinSparkle's C libraries (shared and static) with the header and a
# pkg-config file, link the C and C++ smoke tests against each and run them,
# and pack it all as dist/linsparkle-<version>-<target>.tar.gz.
#
#     scripts/build-c-libs.sh [cargo-target]
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=${1:-$(rustc -vV | sed -n 's/^host: //p')}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
NAME="linsparkle-$VERSION-$TARGET"
OUT="dist/$NAME"
# --color never: the library list below is read from rustc's output, and
# CARGO_TERM_COLOR=always (as CI sets it) would wrap it in escape codes.
CARGO=(cargo rustc --color never --release --lib --features capi --target "$TARGET")
rm -rf "$OUT" && mkdir -p "$OUT/include" "$OUT/lib/pkgconfig"

# Static library, and the system libraries anything linking it also needs
# (rustc prints them only when it actually builds, so force that).
cargo clean --release --target "$TARGET" -p linsparkle
"${CARGO[@]}" --crate-type staticlib -- --print native-static-libs 2> "$OUT/build.log" || {
  cat "$OUT/build.log" >&2
  exit 1
}
NATIVE_LIBS=$(sed -n 's/.*native-static-libs: //p' "$OUT/build.log" | tail -1)
rm "$OUT/build.log"
test -n "$NATIVE_LIBS"
read -r -a NATIVE <<< "$NATIVE_LIBS"
STATIC="target/$TARGET/release/liblinsparkle.a"

# Shared library, linked from the static one rather than built as a Rust
# cdylib: a cdylib exports every #[no_mangle] symbol in every crate,
# including Slint's own C symbols (for its C++ API), which would clash with
# an app that also links Slint. Linking it here exports only linsparkle_*.
# Linux gets a soname and macOS an @rpath install name, so programs record
# a stable name rather than the build path.
CC=${CC:-cc}
CXX=${CXX:-c++}
SYMBOLS=$(grep -E '^[a-z].*\blinsparkle_[a-z_]+\(' include/linsparkle.h | grep -oE 'linsparkle_[a-z_]+\(' | tr -d '(' | sort -u)
EXPORTS=$(mktemp)
case "$TARGET" in
  *apple*)
    SHARED=liblinsparkle.dylib
    sed 's/^/_/' <<< "$SYMBOLS" > "$EXPORTS"
    UNDEFINED=(); for s in $SYMBOLS; do UNDEFINED+=("-Wl,-u,_$s"); done
    $CC -dynamiclib -o "$OUT/lib/$SHARED" -Wl,-install_name,@rpath/$SHARED \
      -Wl,-exported_symbols_list,"$EXPORTS" -Wl,-dead_strip "${UNDEFINED[@]}" "$STATIC" "${NATIVE[@]}"
    ;;
  *)
    SHARED=liblinsparkle.so
    { echo '{ global:'; sed 's/$/;/' <<< "$SYMBOLS"; echo 'local: *; };'; } > "$EXPORTS"
    UNDEFINED=(); for s in $SYMBOLS; do UNDEFINED+=("-Wl,-u,$s"); done
    $CC -shared -o "$OUT/lib/$SHARED" -Wl,-soname,$SHARED -Wl,--version-script="$EXPORTS" \
      -Wl,--gc-sections -Wl,-z,defs "${UNDEFINED[@]}" "$STATIC" "${NATIVE[@]}"
    ;;
esac

cp "$STATIC" "$OUT/lib/"
# Ship both without debug info, most of which is the prebuilt Rust std's.
# Before the smoke tests, so they link what's shipped.
case "$TARGET" in
  *apple*)
    strip -x "$OUT/lib/$SHARED"
    strip -S "$OUT/lib/liblinsparkle.a"
    ;;
  *)
    strip --strip-unneeded "$OUT/lib/$SHARED"
    strip --strip-debug "$OUT/lib/liblinsparkle.a"
    ;;
esac
cp include/linsparkle.h "$OUT/include/"
cp LICENSE-MIT LICENSE-APACHE README.md "$OUT/"
cat > "$OUT/lib/pkgconfig/linsparkle.pc" <<PC
prefix=\${pcfiledir}/../..
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: linsparkle
Description: Sparkle-style updates for Linux AppImages
Version: $VERSION
Libs: -L\${libdir} -llinsparkle
Libs.private: $NATIVE_LIBS
Cflags: -I\${includedir}
PC

# Link the smoke tests both ways and run them. The static builds point at
# the .a by path so the linker can't pick the shared library instead.
TESTS=$(mktemp -d)
trap 'rm -rf "$TESTS" "$EXPORTS"' EXIT
$CC -std=c99 -Wall -Werror -I "$OUT/include" tests/c/smoke.c "$OUT/lib/liblinsparkle.a" "${NATIVE[@]}" -o "$TESTS/c-static"
$CC -std=c99 -Wall -Werror -I "$OUT/include" tests/c/smoke.c -L "$OUT/lib" -Wl,-rpath,"$OUT/lib" -llinsparkle -o "$TESTS/c-shared"
$CXX -std=c++11 -Wall -Werror -I "$OUT/include" tests/c/smoke.cpp "$OUT/lib/liblinsparkle.a" "${NATIVE[@]}" -o "$TESTS/cpp-static"
$CXX -std=c++11 -Wall -Werror -I "$OUT/include" tests/c/smoke.cpp -L "$OUT/lib" -Wl,-rpath,"$OUT/lib" -llinsparkle -o "$TESTS/cpp-shared"
$CC -std=c99 -Wall -Werror -D_DEFAULT_SOURCE -I "$OUT/include" examples/c/demo.c -L "$OUT/lib" -llinsparkle -o "$TESTS/demo"
"$TESTS/c-static"
"$TESTS/cpp-static"
"$TESTS/c-shared"
"$TESTS/cpp-shared"

tar czf "dist/$NAME.tar.gz" -C dist "$NAME"
echo "built dist/$NAME.tar.gz"
