/*
 * LinSparkle: Sparkle-style updates for Linux AppImages.
 *
 * The calls mirror WinSparkle's (win_sparkle_* -> linsparkle_*). Strings are
 * UTF-8 and copied during the call; NULL is ignored.
 *
 *     int main(int argc, char **argv) {
 *         linsparkle_run_if_helper();   // first: the update UI runs as a copy
 *                                       // of this executable and branches here
 *         linsparkle_set_appcast_url("https://example.com/appcast/linux.xml");
 *         linsparkle_set_eddsa_public_key("<base64 Ed25519 public key>");
 *         linsparkle_set_app_details("My App", "1.2.0");
 *         linsparkle_set_shutdown_request_callback(quit_my_app);
 *         linsparkle_init();
 *         ...
 *         linsparkle_cleanup();
 *     }
 *
 * Licensed under MIT OR Apache-2.0.
 */
#ifndef LINSPARKLE_H
#define LINSPARKLE_H

#ifdef __cplusplus
extern "C" {
#endif

/* Call first thing in main(). In LinSparkle's helper process this runs the
 * update UI and exits; otherwise it returns at once. */
void linsparkle_run_if_helper(void);

/* The Sparkle appcast to check. Items whose enclosure has a sparkle:os other
 * than "linux" are ignored. */
void linsparkle_set_appcast_url(const char *url);

/* Base64 Ed25519 public key (Sparkle's SUPublicEDKey). Every update must
 * carry a matching sparkle:edSignature. Returns 1 if the key is valid, else 0
 * (and the previous key is kept). */
int linsparkle_set_eddsa_public_key(const char *pubkey);

/* The app's display name and its version as the appcast's sparkle:version
 * numbers it. */
void linsparkle_set_app_details(const char *app_name, const char *app_version);

/* PNG for the update window. Defaults to the AppImage's icon ($APPDIR/.DirIcon). */
void linsparkle_set_app_icon(const char *path);

/* The AppImage to update in place. Defaults to $APPIMAGE; without either,
 * updates open in the browser. */
void linsparkle_set_install_path(const char *path);

/* Bundle mode (since 0.2): one update replaces a suite of AppImages
 * installed side by side. Off (0) by default. When on, a .tar.gz update is a
 * bundle of several AppImages, optionally with a linsparkle-bundle.json
 * manifest: the running AppImage is replaced by its own member, and every
 * other member replaces the file of the same name in the same directory,
 * only if that file is already there. Nothing is replaced unless every file
 * could be staged first. If the directory isn't writable, the update is
 * offered for download instead. */
void linsparkle_set_bundle_mode(int enabled);

/* In bundle mode, this app's "id" in the bundle's manifest, so its member is
 * found even if its AppImage was renamed. Otherwise the member with the
 * running AppImage's file name is used. "" clears it. */
void linsparkle_set_bundle_app_id(const char *id);

/* Check automatically from linsparkle_init(), once per interval. Off by default. */
void linsparkle_set_automatic_check_for_updates(int state);

/* Seconds between automatic checks; at least 3600. Default 86400. */
void linsparkle_set_update_check_interval(int interval);

/* Called on a LinSparkle thread once an update is installed and will start
 * when this process exits: the app should quit. Without a callback the
 * update starts the next time the app is opened. */
typedef void (*linsparkle_shutdown_request_callback_t)(void);
void linsparkle_set_shutdown_request_callback(linsparkle_shutdown_request_callback_t callback);

/* Start LinSparkle; runs an automatic check if they're on and one is due. */
void linsparkle_init(void);

/* Check now and show the result, including "up to date" and errors. */
void linsparkle_check_update_with_ui(void);

/* Check quietly; only shows a window for an update that wasn't skipped. */
void linsparkle_check_update_without_ui(void);

/* 1 while a check or the update window is running, else 0. */
int linsparkle_is_checking(void);

/* Stop any running check or update window. Call before exiting. */
void linsparkle_cleanup(void);

#ifdef __cplusplus
}
#endif

#endif /* LINSPARKLE_H */
