/* Links against LinSparkle and exercises the calls that need no display or
 * network. Built and run by scripts/build-c-libs.sh, statically and shared. */
#include <linsparkle.h>
#include <stdio.h>

static void on_shutdown(void) {}

int main(void) {
    linsparkle_run_if_helper();
    if (!linsparkle_set_eddsa_public_key("1cinnYnfnTig5q3P1zMY+VMsmPkllo7kRYu9NpRuPw4=")) {
        fprintf(stderr, "a valid key was refused\n");
        return 1;
    }
    if (linsparkle_set_eddsa_public_key("not a key") || linsparkle_set_eddsa_public_key(NULL)) {
        fprintf(stderr, "an invalid key was accepted\n");
        return 1;
    }
    linsparkle_set_appcast_url("https://example.com/appcast.xml");
    linsparkle_set_app_details("Smoke Test", "1.0.0");
    linsparkle_set_app_icon(NULL);
    linsparkle_set_install_path("/nonexistent/Smoke.AppImage");
    linsparkle_set_update_check_interval(60);
    linsparkle_set_automatic_check_for_updates(0);
    linsparkle_set_shutdown_request_callback(on_shutdown);
    linsparkle_init();
    if (linsparkle_is_checking()) {
        fprintf(stderr, "checking without being asked\n");
        return 1;
    }
    linsparkle_cleanup();
    puts("linsparkle C smoke test: ok");
    return 0;
}
