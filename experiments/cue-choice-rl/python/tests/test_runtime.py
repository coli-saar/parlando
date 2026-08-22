"""Dependency-light tests for checkpoint and update semantics."""

from __future__ import annotations

import pytest

from cue_choice_rl.qwen_backend import QwenPpoBackend
from cue_choice_rl.runtime import LearnerRuntime, RandomBackend, canonical_prompt


class _Availability:
    """Minimal accelerator availability API used by the device-selection tests."""

    def __init__(self, available: bool) -> None:
        """Stores the availability result returned to the backend."""
        self._available = available

    def is_available(self) -> bool:
        """Returns the configured accelerator availability."""
        return self._available


class _FakeTorch:
    """Small torch stand-in exposing CUDA, MPS, and device construction."""

    def __init__(self, *, cuda: bool, mps: bool) -> None:
        """Builds fake accelerator namespaces from explicit availability flags."""
        self.cuda = _Availability(cuda)
        self.backends = type("Backends", (), {"mps": _Availability(mps)})()

    @staticmethod
    def device(name: str) -> str:
        """Returns the requested device name for direct assertions."""
        return name


def _backend_with_devices(*, cuda: bool, mps: bool) -> QwenPpoBackend:
    """Constructs an unloaded backend with a controlled torch capability surface."""
    backend = QwenPpoBackend.__new__(QwenPpoBackend)
    backend.torch = _FakeTorch(cuda=cuda, mps=mps)
    return backend


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


@pytest.mark.parametrize(
    ("cuda", "mps", "expected"),
    [(True, True, "cuda"), (False, True, "mps"), (False, False, "cpu")],
)
def test_auto_device_prefers_cuda_then_mps_then_cpu(
    cuda: bool, mps: bool, expected: str
) -> None:
    """Automatic selection uses the documented accelerator priority."""
    assert _backend_with_devices(cuda=cuda, mps=mps)._select_device("auto") == expected


@pytest.mark.parametrize("device", ["cuda", "mps"])
def test_explicit_unavailable_accelerator_fails(device: str) -> None:
    """An explicit accelerator request never falls back silently."""
    with pytest.raises(RuntimeError, match=f"{device.upper()} is unavailable"):
        _backend_with_devices(cuda=False, mps=False)._select_device(device)
