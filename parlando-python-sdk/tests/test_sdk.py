"""Exercise the typed author API against literal v4 wire fixtures."""

from dataclasses import dataclass
import json
import unittest

from parlando_game_sdk import Codec, Game, Rejection, Role, Session, U8, U64, WireError, mechanics, variant


@variant("add")
@dataclass(frozen=True)
class Add:
    """An action carrying one positional counter increment."""

    amount: U8


@variant("pass")
@dataclass(frozen=True)
class Pass:
    """A unit alternative in a mixed enum."""


@variant("bad_view")
@dataclass(frozen=True)
class BadView:
    """An accepted action that exposes a projection bug for the test."""


@dataclass(frozen=True)
class Values:
    """A wire fixture with structured data and explicit integer ranges."""

    signed: int
    unsigned: U64
    maybe: str | None
    items: dict[str, list[tuple[bool, float]]]


Action = Add | Pass | BadView


class Counter(Game[None, Action, int, int, int, int]):
    """Fixture owning a deterministic counter and a private compact checkpoint."""

    def __init__(self, count: int):
        """Start from the seeded or restored counter."""
        self.count = count
        self.broken_view = False

    @classmethod
    def init(cls, config: None, seed: int) -> "Counter":
        """Use the seed as the initial counter."""
        return cls(seed)

    def step(self, actor: Role, action: Action) -> None:
        """Only A can act, and successful actions publish the resulting count."""
        if actor == Role.B:
            raise Rejection("wrong_actor")
        if isinstance(action, Add):
            self.count += action.amount
        elif isinstance(action, BadView):
            self.count += 1
            self.broken_view = True

    def view_a(self) -> int:
        """Return the current count, except when testing a broken projection."""
        return "not a counter" if self.broken_view else self.count

    def view_b(self) -> int:
        """Return the current count from B's perspective."""
        return -self.count

    def completion(self) -> int | None:
        """Report the terminal count once the threshold has been reached."""
        return self.count if self.count >= 10 else None

    def metadata(self) -> int:
        """Report the current count after every accepted action."""
        return self.count

    def checkpoint(self) -> bytes:
        """Use the same compact codec to persist the private counter."""
        return Codec(int).encode(self.count)

    @classmethod
    def restore(cls, checkpoint: bytes) -> "Counter":
        """Restore a previous count or surface a stable checkpoint error."""
        try:
            count = Codec(int).decode(checkpoint)
        except WireError as error:
            raise Rejection("bad_checkpoint") from error
        return cls(count)


class CodecTests(unittest.TestCase):
    """Check schema and typed compact data stay consistent."""

    def test_record_golden_bytes(self):
        """Struct slots, fixed tuples, and signedness match v4 compact MessagePack."""
        codec = Codec(Values)
        value = Values(-129, 2**64 - 1, None, {"a": [(True, 1.5)]})
        encoded = bytes.fromhex("94 d1ff7f cfffffffffffffffff c0 81a1619192 c3 cb3ff8000000000000")
        self.assertEqual(codec.encode(value), encoded)
        self.assertEqual(codec.decode(encoded), value)
        self.assertEqual(codec.schema()["value"][:3], [
            {"name": "signed", "schema": {"type": "i64"}},
            {"name": "unsigned", "schema": {"type": "u64"}},
            {"name": "maybe", "schema": {"type": "option", "value": {"type": "string"}}},
        ])

    def test_enum_golden_bytes(self):
        """Mixed enums use arrays and all-unit enums use bare ordinals."""
        codec = Codec(Add | Pass)
        self.assertEqual(codec.schema(), {
            "type": "enum",
            "value": {
                "representation": {"type": "internal", "value": {"tag": "type"}},
                "variants": [
                    {"name": "add", "payload": {"type": "struct", "value": [
                        {"name": "amount", "schema": {"type": "u64"}},
                    ]}},
                    {"name": "pass", "payload": {"type": "unit"}},
                ],
            },
        })
        self.assertEqual(codec.encode(Add(255)), bytes.fromhex("92 00 ccff"))
        self.assertEqual(codec.decode(bytes.fromhex("92 00 ccff")), Add(255))
        self.assertEqual(codec.encode(Pass()), bytes.fromhex("91 01"))
        self.assertEqual(codec.decode(bytes.fromhex("91 01")), Pass())
        self.assertEqual(Codec(Pass | BadView).encode(BadView()), b"\x01")
        self.assertEqual(Codec(Pass | BadView).decode(b"\x00"), Pass())
        self.assertIsNone(Codec(Add | Pass | None).decode(b"\xc0"))

    def test_invalid_payloads(self):
        """Reject malformed messages, incompatible shapes, and scalar overflow."""
        for annotation, payload in (
            (int, b"\x01\x00"),
            (int, bytes.fromhex("cfffffffffffffffff")),
            (U8, bytes.fromhex("cd0100")),
            (U64, b"\xff"),
            (int, b"\xc3"),
            (bool, b"\x01"),
            (str, bytes.fromhex("a1ff")),
            (float, bytes.fromhex("cb7ff8000000000000")),
            (dict[str, int], bytes.fromhex("82 a16101 a16102")),
            (dict[str, int], bytes.fromhex("81 0102")),
            (list[int], bytes.fromhex("ddffffffff")),
            (list[int], bytes.fromhex("9191") * 65 + b"\x00"),
            (Add | Pass, bytes.fromhex("91 00")),
            (Add | Pass, bytes.fromhex("91 02")),
            (Add | Pass, b"\x01"),
            (Pass | BadView, bytes.fromhex("91 00")),
        ):
            with self.subTest(annotation=annotation, payload=payload), self.assertRaises(WireError):
                Codec(annotation).decode(payload)
        for annotation, value in ((U8, 256), (int, True), (float, float("inf")), (str, "\ud800")):
            with self.subTest(annotation=annotation, value=value), self.assertRaises(WireError):
                Codec(annotation).encode(value)

    def test_encoding_budget(self):
        """Bound shared nested input and oversized UTF-8 output before crossing WIT."""
        with self.assertRaisesRegex(WireError, "complexity"):
            Codec(list[list[int]]).encode([[1] * 1024] * 1024)
        with self.assertRaisesRegex(WireError, "byte limit"):
            Codec(str).encode("é" * (4 * 1024 * 1024))


class SessionTests(unittest.TestCase):
    """Exercise the request lifecycle that is shared by native and WIT sessions."""

    def test_rejections_preserve_state(self):
        """Invalid wire actions and handled rejections leave the session reusable."""
        session = Session(Counter)
        schema = json.loads(session.describe())
        self.assertEqual(schema["action"], Codec(Action).schema())
        with self.assertRaisesRegex(Rejection, "^invalid_component_config$"):
            session.init(b"\x90", 4)
        frame = session.init(b"\xc0", 4)
        self.assertEqual((frame.view_a, frame.view_b, frame.metadata), (b"\x04", b"\xfc", None))
        add = Codec(Action).encode(Add(6))
        with self.assertRaisesRegex(Rejection, "^invalid_component_action$"):
            session.step(Role.A, add[:-1])
        with self.assertRaisesRegex(Rejection, "^wrong_actor$"):
            session.step(Role.B, add)
        frame = session.step(Role.A, add)
        self.assertEqual((frame.view_a, frame.completion, frame.metadata), (b"\x0a",) * 3)

    def test_recovery_after_projection_bug(self):
        """A projection error remains a trap and the last accepted checkpoint can recover."""
        session = Session(Counter)
        frame = session.init(b"\xc0", 2)
        codec = Codec(Action)
        with self.assertRaises(WireError):
            session.step(Role.A, codec.encode(BadView()))
        recovery = Session(Counter)
        with self.assertRaisesRegex(Rejection, "^bad_checkpoint$"):
            recovery.restore(b"")
        recovery.restore(frame.checkpoint)
        accepted = recovery.step(Role.A, codec.encode(Add(3)))
        replay = Session(Counter)
        replay.init(b"\xc0", 2)
        self.assertEqual(accepted, replay.step(Role.A, codec.encode(Add(3))))

    def test_export_class_can_be_defined_without_wit(self):
        """Native imports of app modules can expose Mechanics before WIT bindings exist."""
        self.assertEqual(json.loads(mechanics(Counter)().describe()), json.loads(Session(Counter).describe()))
