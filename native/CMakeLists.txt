cmake_minimum_required(VERSION 3.21)

project(pervue_native VERSION 0.1.0 LANGUAGES C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)

set(PERVUE_HOST_VERSION "${PROJECT_VERSION}-dev")

configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/include/pervue/version.h.in"
  "${CMAKE_CURRENT_BINARY_DIR}/generated/pervue/version.h"
  @ONLY
)

option(PERVUE_ENABLE_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer where supported" OFF)
option(PERVUE_BUILD_FUZZERS "Build libFuzzer targets with Clang" OFF)

include(CMakePushCheckState)
include(CheckCSourceCompiles)

if(PERVUE_ENABLE_SANITIZERS)
  if(NOT CMAKE_C_COMPILER_ID MATCHES "Clang|GNU")
    message(FATAL_ERROR
      "PERVUE_ENABLE_SANITIZERS requires a GCC/Clang toolchain with ASan+UBSan support; "
      "compiler '${CMAKE_C_COMPILER_ID}' is not supported."
    )
  endif()

  unset(PERVUE_HAVE_ASAN_UBSAN CACHE)
  cmake_push_check_state(RESET)
  set(CMAKE_REQUIRED_FLAGS "-fsanitize=address,undefined")
  set(CMAKE_REQUIRED_LINK_OPTIONS "-fsanitize=address,undefined")
  check_c_source_compiles(
    "int main(void) { return 0; }"
    PERVUE_HAVE_ASAN_UBSAN
  )
  cmake_pop_check_state()

  if(NOT PERVUE_HAVE_ASAN_UBSAN)
    message(FATAL_ERROR
      "PERVUE_ENABLE_SANITIZERS requires working ASan+UBSan compiler and runtime support."
    )
  endif()
endif()

if(PERVUE_BUILD_FUZZERS)
  if(MSVC OR NOT CMAKE_C_COMPILER_ID MATCHES "Clang")
    message(FATAL_ERROR "PERVUE_BUILD_FUZZERS requires a Clang/libFuzzer toolchain")
  endif()

  unset(PERVUE_HAVE_LIBFUZZER CACHE)
  cmake_push_check_state(RESET)
  set(CMAKE_REQUIRED_FLAGS "-fsanitize=fuzzer,address,undefined")
  set(CMAKE_REQUIRED_LINK_OPTIONS "-fsanitize=fuzzer,address,undefined")
  check_c_source_compiles(
    "#include <stddef.h>
#include <stdint.h>
int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  (void)data;
  (void)size;
  return 0;
}"
    PERVUE_HAVE_LIBFUZZER
  )
  cmake_pop_check_state()

  if(NOT PERVUE_HAVE_LIBFUZZER)
    message(FATAL_ERROR
      "PERVUE_BUILD_FUZZERS requires working Clang/libFuzzer and sanitizer runtimes."
    )
  endif()
endif()

function(pervue_enable_project_warnings target)
  if(MSVC)
    target_compile_options(${target} PRIVATE /W4 /WX)
  else()
    target_compile_options(${target} PRIVATE -Wall -Wextra -Wpedantic -Werror)
  endif()
endfunction()

function(pervue_enable_sanitizers target)
  if(NOT PERVUE_ENABLE_SANITIZERS)
    return()
  endif()

  target_compile_options(${target} PRIVATE -fsanitize=address,undefined -fno-omit-frame-pointer)

  get_target_property(pervue_target_type ${target} TYPE)
  if(NOT pervue_target_type STREQUAL "STATIC_LIBRARY")
    target_link_options(${target} PRIVATE -fsanitize=address,undefined -fno-omit-frame-pointer)
  endif()
endfunction()

add_library(pervue_host_core STATIC
  framing/frame.c
  protocol/events.c
  protocol/json.c
  protocol/request.c
  protocol/router.c
  host/host.c
)

target_include_directories(pervue_host_core
  PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}/include
    ${CMAKE_CURRENT_BINARY_DIR}/generated
  PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/framing
    ${CMAKE_CURRENT_SOURCE_DIR}/protocol
)

pervue_enable_project_warnings(pervue_host_core)
pervue_enable_sanitizers(pervue_host_core)

add_executable(pervue-host
  host/main.c
)

target_link_libraries(pervue-host PRIVATE pervue_host_core)
pervue_enable_project_warnings(pervue-host)
pervue_enable_sanitizers(pervue-host)

include(CTest)

if(BUILD_TESTING)
  add_executable(pervue-host-startup-test
    tests/host_startup_test.c
  )

  target_link_libraries(pervue-host-startup-test PRIVATE pervue_host_core)
  pervue_enable_project_warnings(pervue-host-startup-test)
  pervue_enable_sanitizers(pervue-host-startup-test)

  add_executable(pervue-frame-test
    tests/frame_test.c
  )

  add_executable(pervue-protocol-test
    tests/protocol_test.c
  )

  target_include_directories(pervue-frame-test
    PRIVATE
      ${CMAKE_CURRENT_SOURCE_DIR}/framing
  )
  target_link_libraries(pervue-frame-test PRIVATE pervue_host_core)
  pervue_enable_project_warnings(pervue-frame-test)
  pervue_enable_sanitizers(pervue-frame-test)

  target_link_libraries(pervue-protocol-test PRIVATE pervue_host_core)
  pervue_enable_project_warnings(pervue-protocol-test)
  pervue_enable_sanitizers(pervue-protocol-test)

  add_test(
    NAME pervue-host-startup
    COMMAND pervue-host-startup-test
  )

  add_test(
    NAME pervue-frame
    COMMAND pervue-frame-test
  )

  add_test(
    NAME pervue-protocol
    COMMAND pervue-protocol-test
  )

  add_test(
    NAME pervue-host-version
    COMMAND
      ${CMAKE_COMMAND}
      "-DPROGRAM=$<TARGET_FILE:pervue-host>"
      "-DARGUMENT=--version"
      "-DEXPECTED_EXIT=0"
      "-DEXPECTED_STDOUT=${PERVUE_HOST_VERSION}"
      -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/check_cli.cmake"
  )

  add_test(
    NAME pervue-host-chrome-origin
    COMMAND
      ${CMAKE_COMMAND}
      "-DPROGRAM=$<TARGET_FILE:pervue-host>"
      "-DARGUMENT=chrome-extension://pervue-test-extension/"
      "-DEXPECTED_EXIT=0"
      "-DIGNORE_STDOUT=ON"
      -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/check_cli.cmake"
  )

  add_test(
    NAME pervue-host-bad-argument
    COMMAND
      ${CMAKE_COMMAND}
      "-DPROGRAM=$<TARGET_FILE:pervue-host>"
      "-DARGUMENT=--bogus"
      "-DEXPECTED_EXIT=64"
      "-DEXPECTED_STDERR_SUBSTRING=usage:"
      -P "${CMAKE_CURRENT_SOURCE_DIR}/tests/check_cli.cmake"
  )
endif()

if(PERVUE_BUILD_FUZZERS)
  add_executable(pervue-frame-fuzz
    fuzz/frame_reader_fuzz.c
    framing/frame.c
  )

  target_include_directories(pervue-frame-fuzz
    PRIVATE
      ${CMAKE_CURRENT_SOURCE_DIR}/include
      ${CMAKE_CURRENT_SOURCE_DIR}/framing
  )

  pervue_enable_project_warnings(pervue-frame-fuzz)
  target_compile_options(pervue-frame-fuzz PRIVATE
    -fsanitize=fuzzer,address,undefined
    -fno-omit-frame-pointer
  )
  target_link_options(pervue-frame-fuzz PRIVATE
    -fsanitize=fuzzer,address,undefined
    -fno-omit-frame-pointer
  )

  add_executable(pervue-protocol-fuzz
    fuzz/protocol_fuzz.c
    protocol/json.c
    protocol/request.c
  )

  target_include_directories(pervue-protocol-fuzz
    PRIVATE
      ${CMAKE_CURRENT_SOURCE_DIR}/include
      ${CMAKE_CURRENT_SOURCE_DIR}/protocol
  )

  pervue_enable_project_warnings(pervue-protocol-fuzz)
  target_compile_options(pervue-protocol-fuzz PRIVATE
    -fsanitize=fuzzer,address,undefined
    -fno-omit-frame-pointer
  )
  target_link_options(pervue-protocol-fuzz PRIVATE
    -fsanitize=fuzzer,address,undefined
    -fno-omit-frame-pointer
  )
endif()
