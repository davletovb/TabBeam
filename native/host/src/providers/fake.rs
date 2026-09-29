//! The deterministic fake provider (NAT-03 scaffold): it answers every request
//! at once, in process, with a fixed event sequence. Protocol tests and the
//! golden fixtures use it; it starts no processes.

use std::borrow::Cow;
use std::time::Duration;

use super::{Exchange, Provider, Scripted, SendRequest, Timeouts, Update};
use crate::protocol::events::{
    Authentication, Availability, Capabilities, Capability, ProviderState,
};

pub const ID: &str = "fake";

/// The conversation every new fake conversation gets.
pub const CONVERSATION_ID: &str = "fake-conversation";

/// The fake provider's whole answer.
pub const ANSWER: &str = "Fake provider response.";

pub const STATUS: ProviderState = ProviderState {
    availability: Availability::Available,
    authentication: Authentication::Authenticated,
    capabilities: Capabilities {
        streaming: Capability::Supported,
        continuation: Capability::Supported,
        web_search: Capability::Unsupported,
        page_context: Capability::Supported,
        attachments: Capability::Unsupported,
        model_selection: Capability::Unsupported,
        cancellation: Capability::Unsupported,
    },
    models: Cow::Borrowed(&[]),
    sign_in: None,
};

pub struct Fake;

impl Provider for Fake {
    fn id(&self) -> &str {
        ID
    }

    fn capabilities(&self) -> Capabilities {
        STATUS.capabilities
    }

    fn timeouts(&self) -> Timeouts {
        Timeouts {
            start: Duration::from_secs(5),
            idle: Duration::from_secs(5),
            max_turn: Duration::MAX,
            stop_grace: Duration::ZERO,
        }
    }

    fn status(&self) -> Box<dyn Exchange> {
        Box::new(Scripted::new([
            Update::Status {
                provider_id: ID.to_owned(),
                status: STATUS,
            },
            Update::Completed,
        ]))
    }

    fn send(&self, request: SendRequest) -> Box<dyn Exchange> {
        let mut updates = Vec::new();
        let conversation_id = match request.conversation_id {
            Some(conversation_id) => conversation_id,
            None => {
                updates.push(Update::ConversationCreated(CONVERSATION_ID.to_owned()));
                CONVERSATION_ID.to_owned()
            }
        };
        updates.extend([
            Update::Started {
                conversation_id: Some(conversation_id),
            },
            Update::Delta(ANSWER.to_owned()),
            Update::Completed,
        ]);
        Box::new(Scripted::new(updates))
    }
}
