"""Public Python API for implementing Parlando remote agents.

Implementations import the agent, decision, and learner types here. The service registration
functions attach these callbacks to a caller-owned gRPC server. The serve helper runs the
agent service for standalone deployments.
"""

from .server import (
    Agent,
    Context,
    Decision,
    Learner,
    Message,
    Observation,
    PlayerRole,
    Reason,
    Response,
    SessionLogger,
    Transition,
    WakeupRequest,
    add_agent_service,
    add_learner_service,
    serve,
)

__all__ = [
    "Agent",
    "Context",
    "Decision",
    "Learner",
    "Message",
    "Observation",
    "PlayerRole",
    "Reason",
    "Response",
    "SessionLogger",
    "Transition",
    "WakeupRequest",
    "add_agent_service",
    "add_learner_service",
    "serve",
]
