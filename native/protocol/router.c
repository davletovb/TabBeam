#include "pervue/protocol.h"

int pervue_router_dispatch(
    const pervue_router_handlers_t *handlers,
    void *context,
    FILE *output,
    const pervue_request_t *request) {
  pervue_route_handler_t handler = NULL;

  if (handlers == NULL || output == NULL || request == NULL) {
    return 1;
  }

  switch (request->method) {
    case PERVUE_METHOD_CONVERSATION_SEND:
      handler = handlers->conversation_send;
      break;
    case PERVUE_METHOD_PROVIDER_STATUS:
      handler = handlers->provider_status;
      break;
    case PERVUE_METHOD_REQUEST_CANCEL:
      handler = handlers->request_cancel;
      break;
    default:
      return 1;
  }

  if (handler == NULL) {
    return 1;
  }

  return handler(context, output, request);
}
