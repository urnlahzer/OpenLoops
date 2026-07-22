//! The application-layer `ReminderAdapter` port, wired over an injected
//! [`TodoTransport`] — implementation-plan §2.2's "propose/create/reconcile
//! operations" port, filled in for the Microsoft To Do adapter shape.
//!
//! [`TodoTransport`] is the one seam capable of talking to a real Microsoft
//! To Do endpoint; per `todo_boundary` and `G-TODO`, this crate ships no
//! implementation of it — only [`TodoReminderAdapter`], which composes any
//! injected `TodoTransport` into `openloops_application::ports::
//! ReminderAdapter`, and the synthetic doubles under this module's own
//! tests. [`TodoReminderAdapter::attempt`] makes *at most one* transport
//! call per invocation and performs no retry, replay, or durability logic of
//! its own — `openloops_application::ledger`'s `begin_or_replay`/
//! `commit_eligible_retry`/`classify_outcome` state machine remains the sole
//! authority for those decisions (`operation_protocol.durability_order`:
//! "make at most one adapter request"); this port is only ever reached after
//! that module's own commit-before-request step, exactly like every other
//! `ReminderAdapter` implementor `ports.rs` anticipates.
//!
//! `remote_marker_protocol.request_rule`: "send the derived marker only
//! through an endpoint-supported contract-tested opaque field; never a
//! title body due URL or link label." [`TodoWriteRequest`] enforces this
//! structurally: [`TodoMarkerField`] is a distinct type from
//! [`TodoFieldValue`], so a caller cannot place a derived marker into the
//! title/body/due slot without a compile error, and vice versa.

use openloops_application::ports::{
    AdapterOutcome, AdapterRequest, AdapterTransportError, OperationKind, ReminderAdapter,
};

use crate::todo::fields::FieldKind;
use crate::todo::marker::derive_marker;

/// One service-owned field value this attempt writes (title/body/due).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TodoFieldValue {
    pub field: FieldKind,
    pub value: String,
}

/// The one contract-tested opaque field the derived remote marker may
/// occupy (`remote_marker_protocol.request_rule`). A distinct type from
/// [`TodoFieldValue`] so the marker can never be constructed into a
/// title/body/due slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TodoMarkerField(pub String);

/// The one bounded request `TodoTransport::attempt` sends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TodoWriteRequest {
    pub operation_kind: OperationKind,
    /// Service-owned field values only; never the marker.
    pub field_values: Vec<TodoFieldValue>,
    /// The opaque marker field slot; never a title/body/due/URL value.
    pub marker_field: Option<TodoMarkerField>,
}

/// The result of one bounded [`TodoTransport::attempt`] call, mirroring
/// `openloops_application::ports::AdapterOutcome` exactly (this module
/// translates between the two at the [`ReminderAdapter`] boundary rather
/// than reusing the application type directly, so this crate's transport
/// seam never depends on the application crate's type evolving in lockstep
/// by accident).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TodoTransportOutcome {
    Succeeded { destination_locator: Vec<u8> },
    KnownNoSend,
    Ambiguous,
    DefinitiveFailure,
}

/// A transport-level failure to even reach the endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TodoTransportError {
    Unavailable,
}

/// The one seam capable of opening a real Microsoft To Do connection. The
/// real endpoint is `unresolved_pending_G-TODO`; this crate ships no
/// implementation beyond the synthetic doubles under this module's own
/// tests.
pub trait TodoTransport {
    /// Sends one bounded request.
    ///
    /// # Errors
    ///
    /// Returns [`TodoTransportError`] when the endpoint cannot be reached at
    /// all (never a partial/ambiguous result — that is
    /// [`TodoTransportOutcome::Ambiguous`]).
    fn attempt(
        &mut self,
        request: &TodoWriteRequest,
    ) -> Result<TodoTransportOutcome, TodoTransportError>;
}

/// Composes any [`TodoTransport`] into the application layer's
/// [`ReminderAdapter`] port.
pub struct TodoReminderAdapter<T> {
    pub transport: T,
}

impl<T> TodoReminderAdapter<T> {
    #[must_use]
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: TodoTransport> ReminderAdapter for TodoReminderAdapter<T> {
    fn attempt(
        &mut self,
        request: &AdapterRequest,
    ) -> Result<AdapterOutcome, AdapterTransportError> {
        // `request.destination_hint` carries the same opaque, encrypted
        // operation-identity bytes `openloops_application::ledger` already
        // tracks; this port derives the recoverable marker from it and
        // places it only in `marker_field`, never alongside a title/body/due
        // value (there is none to place it beside here at all).
        let marker = derive_marker(&request.destination_hint);
        let wire_request = TodoWriteRequest {
            operation_kind: request.operation_kind,
            field_values: Vec::new(),
            marker_field: Some(TodoMarkerField(marker)),
        };
        match self.transport.attempt(&wire_request) {
            Ok(TodoTransportOutcome::Succeeded {
                destination_locator,
            }) => Ok(AdapterOutcome::Succeeded {
                destination_locator,
            }),
            Ok(TodoTransportOutcome::KnownNoSend) => Ok(AdapterOutcome::KnownNoSend),
            Ok(TodoTransportOutcome::Ambiguous) => Ok(AdapterOutcome::Ambiguous),
            Ok(TodoTransportOutcome::DefinitiveFailure) => Ok(AdapterOutcome::DefinitiveFailure),
            Err(TodoTransportError::Unavailable) => Err(AdapterTransportError::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TodoReminderAdapter, TodoTransport, TodoTransportError, TodoTransportOutcome,
        TodoWriteRequest,
    };
    use openloops_application::ports::{
        AdapterOutcome, AdapterRequest, AdapterTransportError, OperationKind, ReminderAdapter,
    };

    /// Records every request it sees and returns one fixed outcome; used to
    /// assert both the translated [`AdapterOutcome`] and the exact wire
    /// shape (marker placement, call count) `TodoReminderAdapter` builds.
    struct RecordingTransport {
        outcome: Result<TodoTransportOutcome, TodoTransportError>,
        calls: Vec<TodoWriteRequest>,
    }

    impl TodoTransport for RecordingTransport {
        fn attempt(
            &mut self,
            request: &TodoWriteRequest,
        ) -> Result<TodoTransportOutcome, TodoTransportError> {
            self.calls.push(request.clone());
            self.outcome.clone()
        }
    }

    fn request() -> AdapterRequest {
        AdapterRequest {
            operation_kind: OperationKind::TodoCreate,
            destination_hint: vec![7u8; 16],
        }
    }

    #[test]
    fn success_translates_and_carries_the_destination_locator() {
        let mut adapter = TodoReminderAdapter::new(RecordingTransport {
            outcome: Ok(TodoTransportOutcome::Succeeded {
                destination_locator: vec![9, 9],
            }),
            calls: Vec::new(),
        });
        let outcome = adapter.attempt(&request()).expect("transport reachable");
        assert_eq!(
            outcome,
            AdapterOutcome::Succeeded {
                destination_locator: vec![9, 9]
            }
        );
        assert_eq!(
            adapter.transport.calls.len(),
            1,
            "at most one bounded request"
        );
    }

    #[test]
    fn known_no_send_ambiguous_and_definitive_failure_all_translate() {
        for (transport_outcome, expected) in [
            (
                TodoTransportOutcome::KnownNoSend,
                AdapterOutcome::KnownNoSend,
            ),
            (TodoTransportOutcome::Ambiguous, AdapterOutcome::Ambiguous),
            (
                TodoTransportOutcome::DefinitiveFailure,
                AdapterOutcome::DefinitiveFailure,
            ),
        ] {
            let mut adapter = TodoReminderAdapter::new(RecordingTransport {
                outcome: Ok(transport_outcome),
                calls: Vec::new(),
            });
            assert_eq!(adapter.attempt(&request()).unwrap(), expected);
        }
    }

    #[test]
    fn transport_unavailable_translates_to_adapter_transport_error() {
        let mut adapter = TodoReminderAdapter::new(RecordingTransport {
            outcome: Err(TodoTransportError::Unavailable),
            calls: Vec::new(),
        });
        assert_eq!(
            adapter.attempt(&request()),
            Err(AdapterTransportError::Unavailable)
        );
    }

    #[test]
    fn the_marker_lands_only_in_the_opaque_marker_field_never_in_field_values() {
        let mut transport = RecordingTransport {
            outcome: Ok(TodoTransportOutcome::KnownNoSend),
            calls: Vec::new(),
        };
        let mut adapter = TodoReminderAdapter::new(&mut transport);
        adapter.attempt(&request()).unwrap();
        let sent = &transport.calls[0];
        assert!(sent.field_values.is_empty());
        let marker_field = sent.marker_field.as_ref().expect("marker field present");
        assert_eq!(
            marker_field.0,
            crate::todo::marker::derive_marker(&[7u8; 16])
        );
    }

    #[test]
    fn a_single_attempt_call_makes_exactly_one_bounded_transport_request() {
        // The port itself must never loop or retry internally: durability
        // and retry eligibility are `openloops_application::ledger`'s job
        // alone.
        let mut adapter = TodoReminderAdapter::new(RecordingTransport {
            outcome: Ok(TodoTransportOutcome::Ambiguous),
            calls: Vec::new(),
        });
        adapter.attempt(&request()).unwrap();
        adapter.attempt(&request()).unwrap();
        assert_eq!(
            adapter.transport.calls.len(),
            2,
            "two attempts, two requests: no hidden retry loop inside one attempt"
        );
    }

    // `&mut RecordingTransport` needs `TodoTransport` too, for the
    // marker-placement test above to observe the underlying recorder after
    // `adapter` is dropped.
    impl TodoTransport for &mut RecordingTransport {
        fn attempt(
            &mut self,
            request: &TodoWriteRequest,
        ) -> Result<TodoTransportOutcome, TodoTransportError> {
            (**self).attempt(request)
        }
    }
}
