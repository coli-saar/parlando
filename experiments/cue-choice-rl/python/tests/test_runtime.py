"""Dependency-light tests for checkpoint and update semantics."""

from __future__ import annotations

import pytest

from cue_choice_rl.runtime import LearnerRuntime, RandomBackend, canonical_prompt


@pytest.mark.asyncio
async def test_random_backend_is_checkpointed_and_idempotent() -> None:
    """Replaying the same update returns the same immutable checkpoint."""
    runtime = LearnerRuntime(RandomBackend())
    first = await runtime.train("update-1", "initial", 1, [], {})
    second = await runtime.train("update-1", "initial", 1, [], {})
    assert first == second == "cue-random/000001"


@pytest.mark.asyncio
async def test_update_id_rejects_different_payload() -> None:
    """An update ID cannot silently identify two different training batches."""
    runtime = LearnerRuntime(RandomBackend())
    await runtime.train("update-1", "initial", 1, [{"value": "first"}], {})
    with pytest.raises(ValueError, match="reused"):
        await runtime.train("update-1", "initial", 1, [{"value": "second"}], {})


def test_prompt_is_canonical() -> None:
    """Observation key order cannot change the Qwen input text."""
    assert canonical_prompt({"b": 2, "a": 1}) == canonical_prompt({"a": 1, "b": 2})
