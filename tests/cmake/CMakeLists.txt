# The C and C++ smoke tests built through find_package(linsparkle), against
# both targets. scripts/build-c-libs.sh configures this with
# CMAKE_PREFIX_PATH at the package it just built, and runs the results.
cmake_minimum_required(VERSION 3.16)
project(linsparkle_cmake_smoke C CXX)

set(CMAKE_C_STANDARD 99)
set(CMAKE_CXX_STANDARD 11)

find_package(linsparkle ${LINSPARKLE_VERSION} REQUIRED)

# cmake-c-<kind> and cmake-cpp-<kind>, linked against <target>.
function(smoke kind target)
  foreach(lang c cpp)
    add_executable(cmake-${lang}-${kind} ../c/smoke.${lang})
    target_compile_options(cmake-${lang}-${kind} PRIVATE -Wall -Werror)
    target_link_libraries(cmake-${lang}-${kind} PRIVATE ${target})
  endforeach()
endfunction()

smoke(static linsparkle::static)
smoke(shared linsparkle::linsparkle)
