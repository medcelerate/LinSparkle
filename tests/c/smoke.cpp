// The header from C++: extern "C" linkage, and a lambda as the callback.
#include <linsparkle.h>
#include <cstdio>

int main() {
    linsparkle_run_if_helper();
    if (!linsparkle_set_eddsa_public_key("1cinnYnfnTig5q3P1zMY+VMsmPkllo7kRYu9NpRuPw4=")) {
        std::fprintf(stderr, "a valid key was refused\n");
        return 1;
    }
    linsparkle_set_app_details("Smoke Test", "1.0.0");
    linsparkle_set_shutdown_request_callback([] {});
    linsparkle_init();
    if (linsparkle_is_checking()) {
        std::fprintf(stderr, "checking without being asked\n");
        return 1;
    }
    linsparkle_cleanup();
    std::puts("linsparkle C++ smoke test: ok");
    return 0;
}
