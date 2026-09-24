//! Dispatches validated requests to per-method handlers.

use std::io::Write;

use super::events::EventError;
use super::json::JsonStr;
use super::request::{Method, Request, RequestId};

/// Handlers for the v1 methods. Each handler writes the request's events.
pub trait Handlers {
    fn conversation_send<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        provider_id: JsonStr<'_>,
        conversation_id: Option<JsonStr<'_>>,
    ) -> Result<(), EventError>;

    fn provider_status<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        provider_id: Option<JsonStr<'_>>,
    ) -> Result<(), EventError>;

    fn request_cancel<W: Write + ?Sized>(
        &mut self,
        output: &mut W,
        request_id: RequestId<'_>,
        target_request_id: RequestId<'_>,
    ) -> Result<(), EventError>;
}

/// Routes `request` to the handler for its method.
pub fn dispatch<H: Handlers, W: Write + ?Sized>(
    handlers: &mut H,
    output: &mut W,
    request: &Request<'_>,
) -> Result<(), EventError> {
    match request.method {
        Method::ConversationSend {
            provider_id,
            conversation_id,
        } => handlers.conversation_send(output, request.request_id, provider_id, conversation_id),
        Method::ProviderStatus { provider_id } => {
            handlers.provider_status(output, request.request_id, provider_id)
        }
        Method::RequestCancel { target_request_id } => {
            handlers.request_cancel(output, request.request_id, target_request_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::request::parse_request;

    #[derive(Default)]
    struct Probe {
        conversation_calls: usize,
        status_calls: usize,
        cancel_calls: usize,
    }

    impl Handlers for Probe {
        fn conversation_send<W: Write + ?Sized>(
            &mut self,
            _output: &mut W,
            _request_id: RequestId<'_>,
            provider_id: JsonStr<'_>,
            _conversation_id: Option<JsonStr<'_>>,
        ) -> Result<(), EventError> {
            assert!(provider_id.equals_ascii("fake"));
            self.conversation_calls += 1;
            Ok(())
        }

        fn provider_status<W: Write + ?Sized>(
            &mut self,
            _output: &mut W,
            _request_id: RequestId<'_>,
            _provider_id: Option<JsonStr<'_>>,
        ) -> Result<(), EventError> {
            self.status_calls += 1;
            Ok(())
        }

        fn request_cancel<W: Write + ?Sized>(
            &mut self,
            _output: &mut W,
            _request_id: RequestId<'_>,
            _target_request_id: RequestId<'_>,
        ) -> Result<(), EventError> {
            self.cancel_calls += 1;
            Ok(())
        }
    }

    #[test]
    fn dispatches_each_method_to_its_handler() {
        let mut probe = Probe::default();
        let mut output = Vec::new();

        for (method, payload) in [
            (
                "conversation.send",
                r#"{"provider_id":"fake","input":{"text":"Hello"}}"#,
            ),
            ("provider.status", "{}"),
            ("provider.status", r#"{"provider_id":"fake"}"#),
            ("request.cancel", r#"{"target_request_id":"req_1"}"#),
        ] {
            let input = format!(
                r#"{{"version":1,"type":"request","request_id":"req_route","method":"{method}","payload":{payload}}}"#
            );
            let request = parse_request(input.as_bytes()).unwrap();
            dispatch(&mut probe, &mut output, &request).unwrap();
        }

        assert_eq!(
            (
                probe.conversation_calls,
                probe.status_calls,
                probe.cancel_calls
            ),
            (1, 2, 1)
        );
        assert!(output.is_empty());
    }
}
