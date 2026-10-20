# LinSparkle

Sparkle-style updates for Linux AppImages, in Rust.

LinSparkle reads the same appcast and Ed25519 signatures as
[Sparkle](https://sparkle-project.org) (macOS) and
[WinSparkle](https://winsparkle.org) (Windows), so one release pipeline can
feed all three. Its calls mirror WinSparkle's.

## Using it

```rust
fn main() {
    // First thing in main: the update UI runs as this same executable in a
    // helper process, and this is where that process branches off.
    linsparkle::run_if_helper();

    linsparkle::set_appcast_url("https://example.com/appcast/linux.xml");
    linsparkle::set_eddsa_public_key("<base64 Ed25519 public key>");
    linsparkle::set_app_details("My App", env!("CARGO_PKG_VERSION"));
    linsparkle::set_shutdown_request_callback(|| { /* quit the app */ });
    // Optional: inside an AppImage the window uses the AppImage's icon.
    // linsparkle::set_app_icon("/path/to/icon.png");
    linsparkle::set_automatic_check_for_updates(true);
    linsparkle::init(); // checks now if a check is due (default: daily)

    // "Check for Updates…":
    linsparkle::check_update_with_ui();

    // On exit:
    linsparkle::cleanup();
}
```

| WinSparkle | LinSparkle |
| --- | --- |
| `win_sparkle_set_appcast_url` | `set_appcast_url` |
| `win_sparkle_set_eddsa_public_key` | `set_eddsa_public_key` |
| `win_sparkle_set_app_details` | `set_app_details` |
| `win_sparkle_set_automatic_check_for_updates` | `set_automatic_check_for_updates` |
| `win_sparkle_set_update_check_interval` | `set_update_check_interval` |
| `win_sparkle_set_shutdown_request_callback` | `set_shutdown_request_callback` |
| `win_sparkle_init` | `init` (plus `run_if_helper` first in `main`) |
| `win_sparkle_check_update_with_ui` | `check_update_with_ui` |
| `win_sparkle_check_update_without_ui` | `check_update_without_ui` |
| `win_sparkle_cleanup` | `cleanup` |

## From C and C++

Each [release](https://github.com/medcelerate/LinSparkle/releases) has
`linsparkle-<version>-<target>.tar.gz` packages for x86_64 and aarch64 Linux.
Each package holds `include/linsparkle.h`, `lib/liblinsparkle.so`,
`lib/liblinsparkle.a`, a CMake package (`lib/cmake/linsparkle`) and
`lib/pkgconfig/linsparkle.pc`. The functions are
WinSparkle's, renamed from `win_sparkle_*` to `linsparkle_*`. The same header
works from C++.

```c
#include <linsparkle.h>

int main(int argc, char **argv) {
    linsparkle_run_if_helper();   /* first thing in main */
    linsparkle_set_appcast_url("https://example.com/appcast/linux.xml");
    linsparkle_set_eddsa_public_key("<base64 Ed25519 public key>");
    linsparkle_set_app_details("My App", "1.2.0");
    linsparkle_set_shutdown_request_callback(quit_my_app);
    linsparkle_init();
    /* ... */
    linsparkle_cleanup();
}
```

With CMake, point `CMAKE_PREFIX_PATH` at the unpacked package (it works
from wherever it's unpacked), or fetch it:

```cmake
include(FetchContent)
FetchContent_Declare(linsparkle
  URL https://github.com/medcelerate/LinSparkle/releases/download/v0.1.4/linsparkle-0.1.4-x86_64-unknown-linux-gnu.tar.gz
  DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
FetchContent_MakeAvailable(linsparkle)   # or skip these and set CMAKE_PREFIX_PATH
find_package(linsparkle 0.1 REQUIRED PATHS ${linsparkle_SOURCE_DIR})

target_link_libraries(myapp PRIVATE linsparkle::linsparkle)   # shared
# or linsparkle::static, which brings the system libraries it needs
```

`find_package` turns down a package built for another OS or processor, or
from another release series (before 1.0, a different minor version).

```sh
export PKG_CONFIG_PATH=/path/to/linsparkle-<version>-<target>/lib/pkgconfig
cc app.c $(pkg-config --cflags --libs linsparkle)          # shared
cc app.c $(pkg-config --cflags linsparkle) /path/to/lib/liblinsparkle.a \
   $(pkg-config --libs-only-l --static linsparkle | sed 's/-llinsparkle//')   # static
```

The shared library exports only the `linsparkle_*` functions. The static
library has Slint inside it, so don't link it into an app that also links
Slint; use the shared library there. To build the packages yourself, run
`scripts/build-c-libs.sh [target]` (it needs CMake 3.16 or newer). It also
links C and C++ smoke tests against both libraries, directly and through
CMake, and runs them.

## How it works

- **A helper process owns the UI.** The check and the update window run in a
  copy of the app's own executable (`run_if_helper`), with their own Slint
  event loop, so they never touch the app's toolkit or event loop.
- **What gets installed.** The appcast enclosure is either a bare `.AppImage`
  or a `.tar.gz` holding one. Downloads never keep the executable bit, but a
  tarball does. The whole file must match the item's `sparkle:edSignature`
  before anything is written.
- **Replacing the running app.** The new AppImage is staged next to the
  running one (`$APPIMAGE`, or `set_install_path`) and renamed over it.
  Linux allows this: the running copy keeps its inode.
- **Relaunching.** A `/bin/sh` waiter starts the new version once the app
  exits. It isn't run from the AppImage, whose mount goes away on exit. The
  shutdown callback tells the app to quit. Without a callback, the new
  version starts next time.
- **When it can't install in place.** If the app isn't running as an
  AppImage, "Download…" opens the item's `<link>` (or the file) in the
  browser.
- **What it remembers.** Skipped versions and the last check time are kept
  in `$XDG_CONFIG_HOME/linsparkle/<app>/`.

## Appcast

Same format as Sparkle. LinSparkle ignores items whose enclosure has a
`sparkle:os` other than `linux`, so one feed can serve every platform:

```xml
<item>
  <title>Version 1.2.0</title>
  <sparkle:releaseNotesLink>https://example.com/notes/1.2.0.html</sparkle:releaseNotesLink>
  <enclosure sparkle:os="linux" sparkle:version="1.2.0"
             sparkle:edSignature="<base64 signature of the file>"
             url="https://example.com/MyApp-1.2.0.tar.gz" length="0"
             type="application/gzip" />
</item>
```

Sign with Sparkle's `sign_update`, or with openssl:

```sh
openssl pkeyutl -sign -inkey ed25519_priv.pem -rawin -in MyApp-1.2.0.tar.gz | base64 -w0
```

## Try it

```sh
cargo run --example demo -- <appcast-url> <base64-public-key> [current-version] [appimage-to-replace]
```

Building on Linux needs fontconfig (`libfontconfig1-dev`), which Slint uses
for system fonts.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
