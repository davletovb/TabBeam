cmake_minimum_required(VERSION 3.20)

project(pervue_native VERSION 0.1.0 LANGUAGES C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
set(CMAKE_C_EXTENSIONS OFF)

option(PERVUE_ENABLE_SANITIZERS "Enable AddressSanitizer and UndefinedBehaviorSanitizer where supported" OFF)

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

  if(CMAKE_C_COMPILER_ID MATCHES "Clang|GNU")
    target_compile_options(${target} PRIVATE -fsanitize=address,undefined -fno-omit-frame-pointer)
    target_link_options(${target} PRIVATE -fsanitize=address,undefined -fno-omit-frame-pointer)
  else()
    message(WARNING "PERVUE_ENABLE_SANITIZERS is enabled, but ASan+UBSan flags are not configured for ${CMAKE_C_COMPILER_ID}")
  endif()
endfunction()

add_library(pervue_host_core STATIC
  host/host.c
)

target_include_directories(pervue_host_core
  PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}/include
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

  add_test(
    NAME pervue-host-startup
    COMMAND pervue-host-startup-test
  )

  add_test(
    NAME pervue-host-version
    COMMAND pervue-host --version
  )

  set_tests_properties(pervue-host-version PROPERTIES
    PASS_REGULAR_EXPRESSION "^0\\.1\\.0-dev"
  )
endif()
