#ifndef PERVUE_JSON_INTERNAL_H
#define PERVUE_JSON_INTERNAL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define PERVUE_JSON_MAX_DEPTH 128U

typedef enum pervue_json_result {
  PERVUE_JSON_OK = 0,
  PERVUE_JSON_SYNTAX_ERROR = 1,
  PERVUE_JSON_DEPTH_EXCEEDED = 2
} pervue_json_result_t;

typedef struct pervue_json_reader {
  const unsigned char *data;
  size_t length;
  size_t position;
  size_t depth;
} pervue_json_reader_t;

typedef struct pervue_json_string_view {
  const unsigned char *data;
  size_t length;
} pervue_json_string_view_t;

void pervue_json_reader_init(
    pervue_json_reader_t *reader,
    const unsigned char *data,
    size_t length);

void pervue_json_skip_whitespace(pervue_json_reader_t *reader);
bool pervue_json_at_end(const pervue_json_reader_t *reader);
int pervue_json_peek(const pervue_json_reader_t *reader);
bool pervue_json_consume(pervue_json_reader_t *reader, unsigned char expected);

pervue_json_result_t pervue_json_parse_string(
    pervue_json_reader_t *reader,
    pervue_json_string_view_t *view);

pervue_json_result_t pervue_json_parse_integer(
    pervue_json_reader_t *reader,
    int64_t *value,
    bool *is_integer);

pervue_json_result_t pervue_json_skip_value(pervue_json_reader_t *reader);

bool pervue_json_string_equals_ascii(
    const pervue_json_string_view_t *view,
    const char *ascii);

bool pervue_json_string_decode_ascii(
    const pervue_json_string_view_t *view,
    char *output,
    size_t output_capacity,
    size_t *output_length);

bool pervue_json_string_is_nonempty(const pervue_json_string_view_t *view);

#endif
