/* The Rust demo, in C:
 *
 *     demo <appcast-url> <base64-public-key> [current-version] [appimage-to-replace]
 *
 * Build against a release package:
 *
 *     cc demo.c $(pkg-config --cflags --libs linsparkle) -o demo
 */
#include <linsparkle.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static void on_shutdown(void) {
    puts("demo: the updater asked us to quit; the new version starts once we exit");
    exit(0);
}

int main(int argc, char **argv) {
    linsparkle_run_if_helper();

    if (argc < 3) {
        fprintf(stderr, "usage: %s <appcast-url> <base64-public-key> [current-version] [appimage-to-replace]\n", argv[0]);
        return 2;
    }
    linsparkle_set_appcast_url(argv[1]);
    if (!linsparkle_set_eddsa_public_key(argv[2]))
        return 2;
    linsparkle_set_app_details("LinSparkle C Demo", argc > 3 ? argv[3] : "1.0.0");
    if (argc > 4)
        linsparkle_set_install_path(argv[4]);
    linsparkle_set_shutdown_request_callback(on_shutdown);

    linsparkle_check_update_with_ui();
    while (linsparkle_is_checking())
        usleep(200 * 1000);
    linsparkle_cleanup();
    return 0;
}
