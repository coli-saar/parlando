//! Translates the canonical ABI to typed agent calls using the game compact codec.

use std::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use crate::{decode, decode_declared, encode_compact, GameValue};

use super::{
    __bindings::exports::parlando::agent::policy as wire, Agent, AgentView, InitContext, Message,
    Observation, RespondReason, Role, Transition, ViewSource, WakeupRequest,
};

/// Holds the policy installed by the canonical ABI's initialization call.
pub struct Session<A>(Option<A>);

impl<A: Agent> Session<A> {
    /// Creates an empty component before Parlando has selected settings and a role.
    pub const fn new() -> Self {
        Self(None)
    }

    /// Publishes the static factory metadata and exact compact payload layouts.
    pub fn describe() -> Result<String, String> {
        let definition = A::definition();
        let fields: serde_json::Value =
            serde_json::from_str(definition.config_fields).map_err(|error| error.to_string())?;
        let description = serde_json::json!({
            "definition": {
                "id": definition.id,
                "name": definition.name,
                "description": definition.description,
                "config_fields": fields,
            },
            "version": definition.version,
            "action": A::Action::schema(),
            "view": A::View::view_schema(),
            "completion": A::Completion::schema(),
        });
        Ok(description.to_string())
    }

    /// Decodes the settings and initializes the instance with the assigned seat.
    pub fn init(&mut self, config: &[u8], role: wire::Role, seed: u64) -> Result<(), String> {
        if self.0.is_some() {
            return Err("policy already initialized".into());
        }
        let context = InitContext {
            config: decode(config)?,
            role: lift_role(role),
            seed,
        };
        self.0 = Some(ready(A::init(context))?);
        Ok(())
    }

    /// Delivers the game component's first compact view to the policy.
    pub fn start(&mut self, view: Vec<u8>) -> Result<(), String> {
        let view =
            A::View::receive(ViewSource::from_wire(view)).map_err(|error| error.to_string())?;
        ready(self.agent()?.start(view))
    }

    /// Decodes only the compact payloads present in the observation and preserves message order.
    pub fn observe(&mut self, event: wire::Observation) -> Result<(), String> {
        let transition = event
            .transition
            .map(|value| {
                Ok::<_, String>(Transition {
                    actor: lift_role(value.actor),
                    action: decode_declared(&value.action)?,
                    view: A::View::receive(ViewSource::from_wire(value.view))
                        .map_err(|error| error.to_string())?,
                })
            })
            .transpose()?;
        let message = event.message.map(|value| Message {
            sender: lift_role(value.sender),
            text: value.text,
        });
        ready(self.agent()?.observe(Observation {
            transition,
            message,
        }))
    }

    /// Runs a decision and encodes any action directly into the game's compact layout.
    pub fn respond(&mut self, reason: wire::RespondReason) -> Result<wire::Response, String> {
        let reason = match reason {
            wire::RespondReason::SessionStarted => RespondReason::SessionStarted,
            wire::RespondReason::AfterObservation => RespondReason::AfterObservation,
            wire::RespondReason::Scheduled => RespondReason::Scheduled,
            wire::RespondReason::WakeupDue => RespondReason::WakeupDue,
            wire::RespondReason::ActionRejected(code) => RespondReason::ActionRejected(code),
        };
        let outcome = ready(self.agent()?.respond(reason))?;
        let action = outcome.action.as_ref().map(encode_compact).transpose()?;
        let wakeup = outcome.wakeup.map(|request| match request {
            WakeupRequest::After(duration) => wire::WakeupRequest::After(wire::Duration {
                seconds: duration.as_secs(),
                nanoseconds: duration.subsec_nanos(),
            }),
            WakeupRequest::Cancel => wire::WakeupRequest::Cancel,
        });
        Ok(wire::Response {
            action,
            message: outcome.message,
            wakeup,
        })
    }

    /// Delivers the game's terminal value with the game's compact decoding rules.
    pub fn finish(&mut self, completion: &[u8]) -> Result<(), String> {
        ready(self.agent()?.finish(decode_declared(completion)?))
    }

    /// Consumes the active instance and runs its final callback.
    pub fn shutdown(&mut self) -> Result<(), String> {
        self.0
            .take()
            .map_or(Ok(()), |mut agent| ready(agent.shutdown()))
    }

    /// Borrows the session instance after it has been initialized.
    fn agent(&mut self) -> Result<&mut A, String> {
        self.0
            .as_mut()
            .ok_or_else(|| "policy is not initialized".into())
    }
}

/// Converts the canonical role enumeration to the shared Rust game role.
fn lift_role(role: wire::Role) -> Role {
    match role {
        wire::Role::A => Role::A,
        wire::Role::B => Role::B,
    }
}

/// Polls an import-free callback once and lowers its result to WIT.
fn ready<T>(future: impl Future<Output = anyhow::Result<T>>) -> Result<T, String> {
    let mut context = Context::from_waker(Waker::noop());
    match pin!(future).poll(&mut context) {
        Poll::Ready(result) => result.map_err(|error| format!("{error:#}")),
        Poll::Pending => Err(
            "WASM agent callbacks must complete in one poll; use a host wakeup for pacing or gRPC for external I/O"
                .into(),
        ),
    }
}
