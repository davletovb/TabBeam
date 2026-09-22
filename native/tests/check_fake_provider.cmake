include(CMakeParseArguments)

if(NOT DEFINED PROGRAM)
  message(FATAL_ERROR "PROGRAM is required")
endif()

if(NOT DEFINED WORK_DIR)
  set(WORK_DIR "${CMAKE_CURRENT_BINARY_DIR}")
endif()

file(MAKE_DIRECTORY "${WORK_DIR}")

string(CONCAT normal_output
  "{\"type\":\"delta\",\"text\":\"alpha\"}\n"
  "{\"type\":\"delta\",\"text\":\" beta\"}\n"
  "{\"type\":\"completed\"}\n"
)

function(run_fake_mode mode)
  set(options EXPECT_TIMEOUT)
  set(oneValueArgs
    EXPECT_EXIT
    EXPECT_STDOUT
    EXPECT_STDOUT_SUBSTRING
    EXPECT_STDERR_SUBSTRING
    EXPECT_SIZE
    INPUT_TEXT
    TIMEOUT
    MIN_SECONDS
  )
  cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "" ${ARGN})

  if(NOT DEFINED ARG_TIMEOUT)
    set(ARG_TIMEOUT 5)
  endif()

  set(input_file "${WORK_DIR}/fake-${mode}.stdin")
  set(output_file "${WORK_DIR}/fake-${mode}.stdout")
  set(error_file "${WORK_DIR}/fake-${mode}.stderr")

  if(DEFINED ARG_INPUT_TEXT)
    file(WRITE "${input_file}" "${ARG_INPUT_TEXT}")
  else()
    file(WRITE "${input_file}" "")
  endif()

  file(WRITE "${output_file}" "")
  file(WRITE "${error_file}" "")

  string(TIMESTAMP started "%s" UTC)
  execute_process(
    COMMAND "${PROGRAM}" --mode "${mode}"
    INPUT_FILE "${input_file}"
    OUTPUT_FILE "${output_file}"
    ERROR_FILE "${error_file}"
    RESULT_VARIABLE actual_exit
    TIMEOUT "${ARG_TIMEOUT}"
  )
  string(TIMESTAMP finished "%s" UTC)

  if(ARG_EXPECT_TIMEOUT)
    if("${actual_exit}" MATCHES "^-?[0-9]+$")
      message(FATAL_ERROR
        "Mode '${mode}' was expected to time out but exited ${actual_exit}"
      )
    endif()

    string(TOLOWER "${actual_exit}" timeout_text)
    if(NOT timeout_text MATCHES "timeout")
      message(FATAL_ERROR
        "Mode '${mode}' failed for an unexpected reason: ${actual_exit}"
      )
    endif()
  else()
    if(NOT DEFINED ARG_EXPECT_EXIT)
      set(ARG_EXPECT_EXIT 0)
    endif()

    if(NOT "${actual_exit}" STREQUAL "${ARG_EXPECT_EXIT}")
      file(READ "${error_file}" actual_stderr)
      message(FATAL_ERROR
        "Mode '${mode}' exit mismatch: expected ${ARG_EXPECT_EXIT}, "
        "got ${actual_exit}; stderr='${actual_stderr}'"
      )
    endif()
  endif()

  if(DEFINED ARG_MIN_SECONDS)
    math(EXPR elapsed "${finished} - ${started}")
    if(elapsed LESS ARG_MIN_SECONDS)
      message(FATAL_ERROR
        "Mode '${mode}' completed too quickly: ${elapsed}s "
        "(minimum ${ARG_MIN_SECONDS}s)"
      )
    endif()
  endif()

  if(DEFINED ARG_EXPECT_SIZE)
    file(SIZE "${output_file}" actual_size)
    if(NOT actual_size EQUAL ARG_EXPECT_SIZE)
      message(FATAL_ERROR
        "Mode '${mode}' stdout size mismatch: expected ${ARG_EXPECT_SIZE}, "
        "got ${actual_size}"
      )
    endif()
  endif()

  if(DEFINED ARG_EXPECT_STDOUT)
    file(READ "${output_file}" actual_stdout)
    if(NOT "${actual_stdout}" STREQUAL "${ARG_EXPECT_STDOUT}")
      message(FATAL_ERROR
        "Mode '${mode}' stdout mismatch: '${actual_stdout}'"
      )
    endif()
  endif()

  if(DEFINED ARG_EXPECT_STDOUT_SUBSTRING)
    file(READ "${output_file}" actual_stdout)
    string(FIND "${actual_stdout}" "${ARG_EXPECT_STDOUT_SUBSTRING}" index)
    if(index EQUAL -1)
      message(FATAL_ERROR
        "Mode '${mode}' stdout does not contain "
        "'${ARG_EXPECT_STDOUT_SUBSTRING}': '${actual_stdout}'"
      )
    endif()
  endif()

  if(DEFINED ARG_EXPECT_STDERR_SUBSTRING)
    file(READ "${error_file}" actual_stderr)
    string(FIND "${actual_stderr}" "${ARG_EXPECT_STDERR_SUBSTRING}" index)
    if(index EQUAL -1)
      message(FATAL_ERROR
        "Mode '${mode}' stderr does not contain "
        "'${ARG_EXPECT_STDERR_SUBSTRING}': '${actual_stderr}'"
      )
    endif()
  endif()
endfunction()

run_fake_mode(
  normal
  EXPECT_EXIT 0
  EXPECT_STDOUT "${normal_output}"
)

run_fake_mode(
  slow
  EXPECT_EXIT 0
  EXPECT_STDOUT "${normal_output}"
  MIN_SECONDS 1
)

run_fake_mode(
  stderr
  EXPECT_EXIT 0
  EXPECT_STDOUT "${normal_output}"
  EXPECT_STDERR_SUBSTRING "deterministic stderr message"
)

run_fake_mode(
  exit-nonzero
  EXPECT_EXIT 42
  EXPECT_STDERR_SUBSTRING "exiting with status 42"
)

run_fake_mode(
  hang
  EXPECT_TIMEOUT
  TIMEOUT 0.5
  EXPECT_STDOUT_SUBSTRING "{\"type\":\"ready\"}"
)

run_fake_mode(
  ignore-cancel
  EXPECT_TIMEOUT
  TIMEOUT 0.5
  INPUT_TEXT "cancel\n"
  EXPECT_STDOUT_SUBSTRING "{\"type\":\"ready\"}"
  EXPECT_STDERR_SUBSTRING "cancellation ignored"
)

run_fake_mode(
  malformed
  EXPECT_EXIT 0
  EXPECT_STDOUT "{not-json\n"
)

run_fake_mode(
  large
  EXPECT_EXIT 0
  EXPECT_SIZE 2097152
)
