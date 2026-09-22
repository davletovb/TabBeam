if(NOT DEFINED PROGRAM)
  message(FATAL_ERROR "PROGRAM is required")
endif()

if(NOT DEFINED EXPECTED_EXIT)
  message(FATAL_ERROR "EXPECTED_EXIT is required")
endif()

set(empty_stdin "${CMAKE_CURRENT_BINARY_DIR}/pervue_cli_empty_stdin")
file(WRITE "${empty_stdin}" "")

if(IGNORE_STDOUT)
  set(output_arguments OUTPUT_QUIET)
else()
  set(output_arguments OUTPUT_VARIABLE actual_stdout)
endif()

if(DEFINED ARGUMENT)
  execute_process(
    COMMAND "${PROGRAM}" "${ARGUMENT}"
    RESULT_VARIABLE actual_exit
    INPUT_FILE "${empty_stdin}"
    ${output_arguments}
    ERROR_VARIABLE actual_stderr
  )
else()
  execute_process(
    COMMAND "${PROGRAM}"
    RESULT_VARIABLE actual_exit
    INPUT_FILE "${empty_stdin}"
    ${output_arguments}
    ERROR_VARIABLE actual_stderr
  )
endif()

if(NOT "${actual_exit}" STREQUAL "${EXPECTED_EXIT}")
  message(FATAL_ERROR
    "Unexpected exit status: expected ${EXPECTED_EXIT}, got ${actual_exit}\n"
    "stdout: ${actual_stdout}\n"
    "stderr: ${actual_stderr}"
  )
endif()

if(DEFINED EXPECTED_STDOUT)
  string(STRIP "${actual_stdout}" actual_stdout_stripped)

  if(NOT "${actual_stdout_stripped}" STREQUAL "${EXPECTED_STDOUT}")
    message(FATAL_ERROR
      "Unexpected stdout: expected '${EXPECTED_STDOUT}', got '${actual_stdout_stripped}'"
    )
  endif()
endif()

if(DEFINED EXPECTED_STDERR_SUBSTRING)
  string(FIND "${actual_stderr}" "${EXPECTED_STDERR_SUBSTRING}" stderr_index)

  if(stderr_index EQUAL -1)
    message(FATAL_ERROR
      "stderr does not contain '${EXPECTED_STDERR_SUBSTRING}': ${actual_stderr}"
    )
  endif()
endif()
