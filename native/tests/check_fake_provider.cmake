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

set(first_line "{\"type\":\"delta\",\"text\":\"alpha\"}\n")

function(run_fake_mode mode)
  set(options EXPECT_TIMEOUT)
  set(oneValueArgs
    NAME
    EXPECT_EXIT
    EXPECT_STDOUT
    EXPECT_STDOUT_SUBSTRING
    EXPECT_STDERR_SUBSTRING
    EXPECT_SIZE
    INPUT_TEXT
    TIMEOUT
  )
  cmake_parse_arguments(ARG "${options}" "${oneValueArgs}" "" ${ARGN})

  if(NOT DEFINED ARG_NAME)
    set(ARG_NAME "${mode}")
  endif()

  if(NOT DEFINED ARG_TIMEOUT)
    set(ARG_TIMEOUT 5)
  endif()

  set(input_file "${WORK_DIR}/fake-${ARG_NAME}.stdin")
  set(output_file "${WORK_DIR}/fake-${ARG_NAME}.stdout")
  set(error_file "${WORK_DIR}/fake-${ARG_NAME}.stderr")

  if(DEFINED ARG_INPUT_TEXT)
    file(WRITE "${input_file}" "${ARG_INPUT_TEXT}")
  else()
    file(WRITE "${input_file}" "")
  endif()

  file(WRITE "${output_file}" "")
  file(WRITE "${error_file}" "")

  execute_process(
    COMMAND "${PROGRAM}" --mode "${mode}"
    INPUT_FILE "${input_file}"
    OUTPUT_FILE "${output_file}"
    ERROR_FILE "${error_file}"
    RESULT_VARIABLE actual_exit
    TIMEOUT "${ARG_TIMEOUT}"
  )

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

function(run_fake_cli_case name)
  set(oneValueArgs EXPECT_EXIT EXPECT_STDERR_SUBSTRING EXPECT_SIZE)
  set(multiValueArgs ARGS)
  cmake_parse_arguments(ARG "" "${oneValueArgs}" "${multiValueArgs}" ${ARGN})

  set(input_file "${WORK_DIR}/cli-${name}.stdin")
  set(output_file "${WORK_DIR}/cli-${name}.stdout")
  set(error_file "${WORK_DIR}/cli-${name}.stderr")
  file(WRITE "${input_file}" "")
  file(WRITE "${output_file}" "")
  file(WRITE "${error_file}" "")

  execute_process(
    COMMAND "${PROGRAM}" ${ARG_ARGS}
    INPUT_FILE "${input_file}"
    OUTPUT_FILE "${output_file}"
    ERROR_FILE "${error_file}"
    RESULT_VARIABLE actual_exit
    TIMEOUT 5
  )

  if(NOT "${actual_exit}" STREQUAL "${ARG_EXPECT_EXIT}")
    message(FATAL_ERROR
      "CLI case '${name}' exit mismatch: expected ${ARG_EXPECT_EXIT}, "
      "got ${actual_exit}"
    )
  endif()

  if(DEFINED ARG_EXPECT_SIZE)
    file(SIZE "${output_file}" actual_size)
    if(NOT actual_size EQUAL ARG_EXPECT_SIZE)
      message(FATAL_ERROR
        "CLI case '${name}' stdout size mismatch: expected ${ARG_EXPECT_SIZE}, "
        "got ${actual_size}"
      )
    endif()
  endif()

  if(DEFINED ARG_EXPECT_STDERR_SUBSTRING)
    file(READ "${error_file}" actual_stderr)
    string(FIND "${actual_stderr}" "${ARG_EXPECT_STDERR_SUBSTRING}" index)
    if(index EQUAL -1)
      message(FATAL_ERROR
        "CLI case '${name}' stderr missing "
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
  NAME slow-complete
  EXPECT_EXIT 0
  EXPECT_STDOUT "${normal_output}"
)

# The slow fixture has two 700 ms pauses, so it must not finish inside 1 s.
# This timeout-based assertion is independent of wall-clock timestamp sources
# such as SOURCE_DATE_EPOCH.
run_fake_mode(
  slow
  NAME slow-timing
  EXPECT_TIMEOUT
  TIMEOUT 1.0
)

# The first line is flushed before the first delay. Killing the process at
# 0.5 s must therefore leave exactly one complete line in the output file.
run_fake_mode(
  slow
  NAME slow-first-flush
  EXPECT_TIMEOUT
  TIMEOUT 0.5
  EXPECT_STDOUT "${first_line}"
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
  EXPECT_SIZE 0
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

run_fake_cli_case(
  no-arguments
  EXPECT_EXIT 64
  EXPECT_SIZE 0
  EXPECT_STDERR_SUBSTRING "usage:"
)

run_fake_cli_case(
  missing-mode-value
  ARGS --mode
  EXPECT_EXIT 64
  EXPECT_SIZE 0
  EXPECT_STDERR_SUBSTRING "usage:"
)

run_fake_cli_case(
  unknown-mode
  ARGS --mode bogus
  EXPECT_EXIT 64
  EXPECT_SIZE 0
  EXPECT_STDERR_SUBSTRING "usage:"
)

run_fake_cli_case(
  reversed-arguments
  ARGS normal --mode
  EXPECT_EXIT 64
  EXPECT_SIZE 0
  EXPECT_STDERR_SUBSTRING "usage:"
)

run_fake_cli_case(
  extra-argument
  ARGS --mode normal extra
  EXPECT_EXIT 64
  EXPECT_SIZE 0
  EXPECT_STDERR_SUBSTRING "usage:"
)
