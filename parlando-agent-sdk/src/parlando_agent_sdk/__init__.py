"""Public Python API for implementing Parlando remote agents.

Implementations import Agent, Context, Response, and Learner here. The service registration
functions attach these callbacks to a caller-owned gRPC server. The serve helper runs the
agent service for standalone deployments.
"""

from .server import (
    Agent,
    Context,
    Learner,
    PlayerRole,
    Response,
    SessionLogger,
    add_agent_service,
    add_learner_service,
    serve,
)

__all__ = [
    "Agent",
    "Context",
    "Learner",
    "PlayerRole",
    "Response",
    "SessionLogger",
    "add_agent_service",
    "add_learner_service",
    "serve",
]
