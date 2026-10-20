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

# The CMake package: find_package(linsparkle) with CMAKE_PREFIX_PATH at the
# unpacked tarball. Paths are relative to the config file, like the .pc's.
# The static target's system libraries are rustc's list as CMake items:
# -lfoo becomes foo, and -framework Foo stays one item.
CMAKE_LIBS=()
for ((i = 0; i < ${#NATIVE[@]}; i++)); do
  case "${NATIVE[$i]}" in
    -framework) CMAKE_LIBS+=("-framework ${NATIVE[$((i + 1))]}"); i=$((i + 1)) ;;
    -l*) CMAKE_LIBS+=("${NATIVE[$i]#-l}") ;;
    *) CMAKE_LIBS+=("${NATIVE[$i]}") ;;
  esac
done
CMAKE_LINK=$(IFS=';'; echo "${CMAKE_LIBS[*]}")
case "$TARGET" in
  *apple*) CMAKE_OS=Darwin; SONAME="@rpath/$SHARED" ;;
  *) CMAKE_OS=Linux; SONAME=$SHARED ;;
esac
case "$TARGET" in
  x86_64-*) CMAKE_ARCHES="x86_64 AMD64 amd64" ;;
  aarch64-*) CMAKE_ARCHES="aarch64 arm64 ARM64" ;;
  *) CMAKE_ARCHES="" ;;
esac
# Before 1.0 a minor release may break things, after it a major one.
MAJOR=${VERSION%%.*}; MINOR=${VERSION#*.}; MINOR=${MINOR%%.*}
if [ "$MAJOR" = 0 ]; then
  SAME_SERIES="PACKAGE_FIND_VERSION_MAJOR EQUAL 0 AND PACKAGE_FIND_VERSION_MINOR EQUAL $MINOR"
else
  SAME_SERIES="PACKAGE_FIND_VERSION_MAJOR EQUAL $MAJOR"
fi
mkdir -p "$OUT/lib/cmake/linsparkle"
cat > "$OUT/lib/cmake/linsparkle/linsparkleConfig.cmake" <<CMAKE
# LinSparkle $VERSION for $TARGET. Defines:
#   linsparkle::linsparkle  the shared library; use this in an app that also links Slint
#   linsparkle::static      the static library, with the system libraries it needs
get_filename_component(_linsparkle_prefix "\${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)
if(NOT TARGET linsparkle::linsparkle)
  add_library(linsparkle::linsparkle SHARED IMPORTED)
  set_target_properties(linsparkle::linsparkle PROPERTIES
    IMPORTED_LOCATION "\${_linsparkle_prefix}/lib/$SHARED"
    IMPORTED_SONAME "$SONAME"
    INTERFACE_INCLUDE_DIRECTORIES "\${_linsparkle_prefix}/include")
endif()
if(NOT TARGET linsparkle::static)
  add_library(linsparkle::static STATIC IMPORTED)
  set_target_properties(linsparkle::static PROPERTIES
    IMPORTED_LOCATION "\${_linsparkle_prefix}/lib/liblinsparkle.a"
    IMPORTED_LINK_INTERFACE_LANGUAGES "C"
    INTERFACE_INCLUDE_DIRECTORIES "\${_linsparkle_prefix}/include"
    INTERFACE_LINK_LIBRARIES "$CMAKE_LINK")
endif()
unset(_linsparkle_prefix)
CMAKE
cat > "$OUT/lib/cmake/linsparkle/linsparkleConfigVersion.cmake" <<CMAKE
# Suitable when built for this OS and processor, and compatible when from
# the same release series and at least as new as the version asked for.
set(PACKAGE_VERSION "$VERSION")
set(PACKAGE_VERSION_COMPATIBLE FALSE)
if(NOT PACKAGE_FIND_VERSION)
  set(PACKAGE_VERSION_COMPATIBLE TRUE)
elseif(PACKAGE_FIND_VERSION VERSION_LESS_EQUAL PACKAGE_VERSION AND $SAME_SERIES)
  set(PACKAGE_VERSION_COMPATIBLE TRUE)
  if(PACKAGE_FIND_VERSION VERSION_EQUAL PACKAGE_VERSION)
    set(PACKAGE_VERSION_EXACT TRUE)
  endif()
endif()
set(_linsparkle_arch "\${CMAKE_SYSTEM_PROCESSOR}")
if(APPLE AND CMAKE_OSX_ARCHITECTURES)
  set(_linsparkle_arch "\${CMAKE_OSX_ARCHITECTURES}")
endif()
set(_linsparkle_suitable FALSE)
if(CMAKE_SYSTEM_NAME STREQUAL "$CMAKE_OS")
  foreach(_linsparkle_ok $CMAKE_ARCHES)
    if(_linsparkle_arch STREQUAL _linsparkle_ok)
      set(_linsparkle_suitable TRUE)
    endif()
  endforeach()
endif()
if(NOT _linsparkle_suitable)
  set(PACKAGE_VERSION "\${PACKAGE_VERSION} ($TARGET)")
  set(PACKAGE_VERSION_UNSUITABLE TRUE)
endif()
unset(_linsparkle_arch)
unset(_linsparkle_ok)
unset(_linsparkle_suitable)
CMAKE

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

# Again through find_package, against both CMake targets; and a version
# from another release series must be turned down.
CMAKE_ARGS=(-DCMAKE_PREFIX_PATH="$PWD/$OUT" -DCMAKE_BUILD_TYPE=Release)
cmake -S tests/cmake -B "$TESTS/cmake" "${CMAKE_ARGS[@]}" -DLINSPARKLE_VERSION="$VERSION" > /dev/null
cmake --build "$TESTS/cmake" > /dev/null
for t in c-static cpp-static c-shared cpp-shared; do "$TESTS/cmake/cmake-$t"; done
# The series itself (find_package(linsparkle 0.2) for 0.2.1) is accepted.
cmake -S tests/cmake -B "$TESTS/cmake-series" "${CMAKE_ARGS[@]}" -DLINSPARKLE_VERSION="$MAJOR.$MINOR" > /dev/null
if cmake -S tests/cmake -B "$TESTS/cmake-other" "${CMAKE_ARGS[@]}" -DLINSPARKLE_VERSION="$((MAJOR + 1)).0" > /dev/null 2>&1; then
  echo "find_package accepted linsparkle $((MAJOR + 1)).0" >&2
  exit 1
fi

tar czf "dist/$NAME.tar.gz" -C dist "$NAME"
echo "built dist/$NAME.tar.gz"
