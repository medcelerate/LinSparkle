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
| — | `set_install_path`, `set_app_icon` |
| — | `set_bundle_mode`, `set_bundle_app_id` ([bundle mode](#bundle-mode)) |

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
    /* Optional: one update for a suite of AppImages (see Bundle mode). */
    /* linsparkle_set_bundle_mode(1); linsparkle_set_bundle_app_id("my-app"); */
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
  URL https://github.com/medcelerate/LinSparkle/releases/download/v0.2.1/linsparkle-0.2.1-x86_64-unknown-linux-gnu.tar.gz
  DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
FetchContent_MakeAvailable(linsparkle)   # or skip these and set CMAKE_PREFIX_PATH
find_package(linsparkle 0.2 REQUIRED PATHS ${linsparkle_SOURCE_DIR})

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
  before anything is written. If a tarball holds several AppImages, the one
  with the running AppImage's file name is installed, else the first; in
  [bundle mode](#bundle-mode) the others are installed too.
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

## Bundle mode

For a suite of apps shipped as AppImages installed side by side, say
`~/Applications/Suite/Suite_Editor-x86_64.AppImage`,
`Suite_Player-x86_64.AppImage` and `Suite_Converter-x86_64.AppImage`: one
update, from any of them, replaces them all, like a macOS suite `.pkg`. It's
off by default; turn it on in each app:

```rust
linsparkle::set_bundle_mode(true);
linsparkle::set_bundle_app_id("editor"); // optional, see below
```

```c
linsparkle_set_bundle_mode(1);
linsparkle_set_bundle_app_id("editor");  /* optional */
```

**The bundle** is the appcast enclosure: a `.tar.gz` holding the AppImages
at its top level and, optionally, a `linsparkle-bundle.json` manifest. The
appcast itself is unchanged, and the whole archive is checked against the
item's `sparkle:edSignature` before anything is written, as always.

```json
{
  "members": [
    { "file": "Suite_Editor-x86_64.AppImage", "id": "editor", "name": "Suite Editor" },
    { "file": "Suite_Player-x86_64.AppImage", "id": "player", "name": "Suite Player" },
    { "file": "Suite_Converter-x86_64.AppImage", "id": "converter", "name": "Suite Converter" }
  ]
}
```

Each member's `file` is the name it's installed under. The rest are
optional:

- `id` finds the running app's member even if its AppImage was renamed (see
  below).
- `name` (since 0.2.1) is what the update window calls the app, as in "It
  also updates Suite Player and Suite Converter." Without one, the window
  uses the file name less `.AppImage`. It's plain text: trimmed, at most 64
  characters, with no control characters, line breaks or bidirectional
  formatting characters; a manifest with any other `name` is refused.
  LinSparkle 0.2.0 ignores it. The running app is called by its
  `set_app_details` name, as before.

Without a manifest every ELF file in the archive is a member, called by its
file name. Build it flat, with the main app first (see
[older versions](#older-versions-and-bundles)):

```sh
tar czf Suite-1.2.0.tar.gz -C build Suite_Editor-x86_64.AppImage Suite_Player-x86_64.AppImage \
    Suite_Converter-x86_64.AppImage linsparkle-bundle.json
```

**What it installs.** In the directory of the running AppImage (`$APPIMAGE`,
or `set_install_path`), and nowhere else:

- The running AppImage is replaced by its own member: the one whose `id` is
  the app's `set_bundle_app_id` (so a renamed AppImage is still found), else
  the one with the same file name, else the only member.
- Every other member replaces the file of the same name in that directory,
  **only if that file is already there** and is a plain file (not a link).
  Apps someone doesn't have aren't installed.
- Everything is staged next to its target first, then renamed over it,
  the running app first. If any file can't be staged, nothing is replaced.
  If the running app can't be replaced, nothing is; if another app can't
  be, the rest still are and the error says which.
- Each installed file is executable (`+x`, plus whatever wider permissions
  the old file had).
- The update window says which other apps the update also replaces, by
  their manifest `name`s.

**Other apps that are open** keep running: renaming over a running AppImage
is fine on Linux, and the new version starts when that app is next opened.
Only the app that installed the update is relaunched.

**When the directory isn't writable**, the update is offered for download
(the item's `<link>`, or the file) instead of installed.

**What a bundle may contain.** It's refused, before anything is written, if
any entry is a link (symbolic or hard), a directory other than the archive's
own `./`, or anything but a regular file; if any name has a `/` or is `..`;
if a name appears twice; or if the manifest lists a name that isn't a plain
file name (no `/`, not starting with `.`), a file the archive doesn't hold
as an executable, or a `name` that isn't plain text (see above). Other files (a README, macOS `._` files) are ignored.

### Older versions and bundles

From 0.2, an app without bundle mode installs just one AppImage from a
bundle: the one named like its own, else the first. LinSparkle 0.1 always
installs the first `.AppImage` in a tarball over the running app, whichever
app that is. So don't serve a bundle to apps built with 0.1, except the one
whose AppImage comes first in the archive.

## Appcast

Same format as Sparkle, for a single AppImage and for a
[bundle](#bundle-mode) alike. LinSparkle ignores items whose enclosure has a
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

For a bundle, `url` is the bundle's `.tar.gz` and the signature is over
the whole archive:

```xml
<enclosure sparkle:os="linux" sparkle:version="1.2.0"
           sparkle:edSignature="<base64 signature of Suite-1.2.0.tar.gz>"
           url="https://example.com/Suite-1.2.0.tar.gz" length="0"
           type="application/gzip" />
```

Sign with Sparkle's `sign_update`, or with openssl:

```sh
openssl pkeyutl -sign -inkey ed25519_priv.pem -rawin -in MyApp-1.2.0.tar.gz | base64 -w0
```

## Try it

```sh
cargo run --example demo -- [--bundle[=<app-id>]] <appcast-url> <base64-public-key> [current-version] [appimage-to-replace]
```

Building on Linux needs fontconfig (`libfontconfig1-dev`), which Slint uses
for system fonts.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
