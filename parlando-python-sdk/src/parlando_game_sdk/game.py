"""Implement the Parlando game lifecycle and expose bindings for a component guest."""

from abc import ABC, abstractmethod
from dataclasses import dataclass
from enum import Enum
import json
from typing import TYPE_CHECKING, Generic, TypeVar, get_args, get_origin

from ._messagepack import BYTE_LIMIT, WireError
from .codec import Codec, _Compiler

if TYPE_CHECKING:
    from wit_world.exports.mechanics import Frame as WitFrame, Role as WitRole

_Config = TypeVar("_Config")
_Action = TypeVar("_Action")
_ViewA = TypeVar("_ViewA")
_ViewB = TypeVar("_ViewB")
_Completion = TypeVar("_Completion")
_Metadata = TypeVar("_Metadata")
_Game = TypeVar("_Game", bound="Game")

_PAYLOADS = ("config", "action", "view_a", "view_b", "completion", "metadata")


class Role(Enum):
    """The participant acting in a game session."""

    A = 0
    B = 1


class Rejection(Exception):
    """An expected game error with a stable public reason code."""

    def __init__(self, code: str):
        """Construct a reason to return to the host."""
        super().__init__(code)
        self.code = code


class Game(ABC, Generic[_Config, _Action, _ViewA, _ViewB, _Completion, _Metadata]):
    """Own one mutable game using six typed Parlando payload declarations.

    Declare a concrete game as a subclass of Game[Config, Action, ViewA,
    ViewB, Completion, Metadata]. Use None for an absent completion or
    metadata shape. The same declaration controls decoding, encoding, and
    the schema returned to the host.
    """

    @classmethod
    @abstractmethod
    def init(cls: type[_Game], config: _Config, seed: int) -> _Game:
        """Construct state from validated wire configuration and the recorded seed."""
        raise NotImplementedError

    @abstractmethod
    def step(self, actor: Role, action: _Action) -> None:
        """Apply the typed action or raise Rejection before mutating session state."""
        raise NotImplementedError

    @abstractmethod
    def view_a(self) -> _ViewA:
        """Return participant A's projection of the current state."""
        raise NotImplementedError

    @abstractmethod
    def view_b(self) -> _ViewB:
        """Return participant B's projection of the current state."""
        raise NotImplementedError

    def completion(self) -> _Completion | None:
        """Return an outcome when the game has reached a terminal state."""
        return None

    def metadata(self) -> _Metadata | None:
        """Return optional metadata about the most recent accepted action."""
        return None

    def checkpoint(self) -> bytes | None:
        """Return a complete opaque snapshot or use deterministic action replay."""
        return None

    @classmethod
    def restore(cls: type[_Game], checkpoint: bytes) -> _Game:
        """Rebuild state from a private snapshot or raise Rejection."""
        raise Rejection("game does not support checkpoints")


@dataclass(frozen=True)
class Frame:
    """One accepted session state encoded for the Parlando component interface."""

    checkpoint: bytes | None
    view_a: bytes
    view_b: bytes
    completion: bytes | None
    metadata: bytes | None


class Session(Generic[_Game]):
    """Run one component lifecycle using raw wire payloads and typed game methods.

    This class requires no generated WIT bindings and can run in a native test.
    Each session owns a separate mutable game. Expected request failures raise
    Rejection. A projection failure is an application error and traps the guest.
    """

    def __init__(self, game: type[_Game]):
        """Compile the game's six declarations and create an empty session slot."""
        for base in getattr(game, "__orig_bases__", ()):
            if get_origin(base) is Game:
                annotations = get_args(base)
                break
        else:
            raise TypeError("expected a subclass of Game with six concrete payload annotations")
        compiler = _Compiler()
        self._codecs = {
            name: Codec(annotation, _compiler=compiler)
            for name, annotation in zip(_PAYLOADS, annotations)
        }
        self._game_type = game
        self._game: _Game | None = None

    def describe(self) -> str:
        """Return the host's JSON schema without constructing any game state."""
        schema = {name: codec.schema() for name, codec in self._codecs.items()}
        description = json.dumps(schema, separators=(",", ":"))
        if len(description.encode("utf-8")) > BYTE_LIMIT:
            raise TypeError("component schema exceeds byte limit")
        return description

    def init(self, config: bytes, seed: int) -> Frame:
        """Replace the session only after valid configuration and a complete frame."""
        try:
            typed_config = self._codecs["config"].decode(config)
        except WireError as error:
            raise Rejection("invalid_component_config") from error
        game = self._game_type.init(typed_config, seed)
        frame = self._frame(game, include_metadata=False)
        self._game = game
        return frame

    def restore(self, checkpoint: bytes) -> None:
        """Replace the session only after a valid checkpoint has been restored."""
        self._game = self._game_type.restore(checkpoint)

    def step(self, actor: Role, action: bytes) -> Frame:
        """Decode before stepping and emit all projections after an accepted action."""
        game = self._game
        if game is None:
            raise RuntimeError("game must be initialized or restored")
        try:
            typed_action = self._codecs["action"].decode(action)
        except WireError as error:
            raise Rejection("invalid_component_action") from error
        game.step(actor, typed_action)
        return self._frame(game, include_metadata=True)

    def _frame(self, game: _Game, *, include_metadata: bool) -> Frame:
        """Project state and turn any rejection during projection into a guest trap."""
        try:
            checkpoint = game.checkpoint()
            if checkpoint is not None and (type(checkpoint) is not bytes or len(checkpoint) > BYTE_LIMIT):
                raise WireError("invalid component checkpoint")
            view_a = self._codecs["view_a"].encode(game.view_a())
            view_b = self._codecs["view_b"].encode(game.view_b())
            completion = game.completion()
            metadata = game.metadata() if include_metadata else None
            return Frame(
                checkpoint,
                view_a,
                view_b,
                self._codecs["completion"].encode(completion) if completion is not None else None,
                self._codecs["metadata"].encode(metadata) if metadata is not None else None,
            )
        except Rejection as error:
            raise RuntimeError("game rejected an accepted frame projection") from error


def mechanics(game: type[_Game]) -> type:
    """Build the WIT export class for an app module's top-level Mechanics name.

    Call this from the module handed to componentize-py. Its generated
    bindings supply the WIT role, frame, and result types. Native tests can
    import the SDK and construct Session directly.
    """
    # componentize-py can create export objects for separate calls. The state
    # belongs to the guest module, which is unique to each component instance.
    session = Session(game)

    class Mechanics:
        """Bridge generated WIT types to one private game session."""

        def describe(self) -> str:
            """Return the game's public schema."""
            return session.describe()

        def init(self, config: bytes, seed: int) -> "WitFrame":
            """Translate expected configuration errors to the WIT result."""
            from componentize_py_types import Err

            try:
                frame = session.init(config, seed)
            except Rejection as error:
                raise Err(error.code) from error
            return _wit_frame(frame)

        def restore(self, checkpoint: bytes) -> None:
            """Translate expected checkpoint errors to the WIT result."""
            from componentize_py_types import Err

            try:
                session.restore(checkpoint)
            except Rejection as error:
                raise Err(error.code) from error

        def step(self, actor: "WitRole", action: bytes) -> "WitFrame":
            """Translate the actor, return an accepted frame, or report a rejection."""
            from componentize_py_types import Err

            try:
                frame = session.step(Role[actor.name], action)
            except Rejection as error:
                raise Err(error.code) from error
            return _wit_frame(frame)

    return Mechanics


def _wit_frame(frame: Frame) -> "WitFrame":
    """Adapt a fully projected native frame to componentize-py's WIT record."""
    from wit_world.exports.mechanics import Frame as WitFrame

    return WitFrame(frame.checkpoint, frame.view_a, frame.view_b, frame.completion, frame.metadata)
