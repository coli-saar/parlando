"""Checkpoint store and policy backends for the cue-choice learner."""

from __future__ import annotations

import asyncio
import hashlib
import json
import random
from pathlib import Path
from typing import Any, Protocol


class PolicyBackend(Protocol):
    """Backend contract shared by the smoke-test and Qwen implementations."""

    def choose(
        self,
        checkpoint_id: str,
        observation: dict[str, Any],
        actions: list[dict[str, Any]],
        seed: int,
        temperature: float,
    ) -> dict[str, Any]:
        """Chooses one action from the runner-provided legal actions."""

    def train(
        self, update_id: str, base_checkpoint_id: str, steps: list[dict[str, Any]], settings: dict[str, Any]
    ) -> str:
        """Applies one idempotently named update and returns a checkpoint ID."""


class RandomBackend:
    """Dependency-light backend for end-to-end protocol and runner smoke tests."""

    def __init__(self) -> None:
        """Creates an empty immutable-checkpoint ledger."""
        self._checkpoints = {"initial"}
        self._updates: dict[str, str] = {}

    def choose(
        self,
        checkpoint_id: str,
        observation: dict[str, Any],
        actions: list[dict[str, Any]],
        seed: int,
        temperature: float,
    ) -> dict[str, Any]:
        """Chooses reproducibly without learning, while checking checkpoint identity."""
        del observation, temperature
        if checkpoint_id not in self._checkpoints:
            raise ValueError(f"unknown checkpoint: {checkpoint_id}")
        if not actions:
            raise ValueError("cue-choice policy received no legal actions")
        return random.Random(seed).choice(actions)

    def train(
        self, update_id: str, base_checkpoint_id: str, steps: list[dict[str, Any]], settings: dict[str, Any]
    ) -> str:
        """Creates a deterministic synthetic checkpoint for the update ID."""
        del steps, settings
        prior = self._updates.get(update_id)
        if prior is not None:
            return prior
        if base_checkpoint_id not in self._checkpoints:
            raise ValueError(f"unknown base checkpoint: {base_checkpoint_id}")
        checkpoint = f"cue-random/{len(self._updates) + 1:06d}"
        self._checkpoints.add(checkpoint)
        self._updates[update_id] = checkpoint
        return checkpoint


class LearnerRuntime:
    """Serializes inference and updates against one checkpoint-aware backend."""

    def __init__(self, backend: PolicyBackend) -> None:
        """Owns a backend and a lock protecting its mutable model state."""
        self.backend = backend
        self.lock = asyncio.Lock()
        self._request_hashes: dict[str, str] = {}

    async def choose(
        self,
        checkpoint_id: str,
        observation: dict[str, Any],
        actions: list[dict[str, Any]],
        seed: int,
        temperature: float,
    ) -> dict[str, Any]:
        """Runs checkpoint loading and inference atomically."""
        async with self.lock:
            return self.backend.choose(
                checkpoint_id, observation, actions, seed, temperature
            )

    async def train(
        self,
        update_id: str,
        base_checkpoint_id: str,
        completed_epochs: int,
        steps: list[dict[str, Any]],
        settings: dict[str, Any],
    ) -> str:
        """Rejects update-ID reuse with different bytes and performs one update."""
        payload = json.dumps([base_checkpoint_id, completed_epochs, steps, settings], sort_keys=True)
        digest = hashlib.sha256(payload.encode()).hexdigest()
        existing = self._request_hashes.get(update_id)
        if existing is not None and existing != digest:
            raise ValueError("update_id was reused with a different training batch")
        async with self.lock:
            checkpoint = self.backend.train(update_id, base_checkpoint_id, steps, settings)
            self._request_hashes[update_id] = digest
            return checkpoint


def canonical_prompt(observation: dict[str, Any]) -> str:
    """Encodes a role-safe observation in a stable, game-specific text format."""
    return (
        "Choose the correct cue-choice action. Observation: "
        + json.dumps(observation, sort_keys=True, separators=(",", ":"))
    )


def resolve_checkpoint_directory(value: str) -> Path:
    """Resolves and creates the configured checkpoint directory without shell expansion."""
    path = Path(value).expanduser().resolve()
    path.mkdir(parents=True, exist_ok=True)
    return path
