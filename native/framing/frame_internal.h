#ifndef PERVUE_FRAME_INTERNAL_H
#define PERVUE_FRAME_INTERNAL_H

#include "pervue/frame.h"

typedef size_t (*pervue_frame_read_callback_t)(
    void *context,
    unsigned char *buffer,
    size_t length);

typedef size_t (*pervue_frame_write_callback_t)(
    void *context,
    const unsigned char *buffer,
    size_t length);

typedef int (*pervue_frame_error_callback_t)(void *context);
typedef int (*pervue_frame_flush_callback_t)(void *context);

typedef struct pervue_frame_io {
  void *context;
  pervue_frame_read_callback_t read;
  pervue_frame_write_callback_t write;
  pervue_frame_error_callback_t has_error;
  pervue_frame_flush_callback_t flush;
} pervue_frame_io_t;

pervue_frame_result_t pervue_frame_read_io(
    pervue_frame_io_t *io,
    pervue_frame_t *frame);

pervue_frame_result_t pervue_frame_write_io(
    pervue_frame_io_t *io,
    const unsigned char *data,
    size_t length);

#endif
