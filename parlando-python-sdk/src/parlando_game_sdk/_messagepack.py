"""Bounded MessagePack primitives for the v4 component's JSON-compatible values."""

import math
import struct

BYTE_LIMIT = 8 * 1024 * 1024
NODE_LIMIT = 262_144
DEPTH_LIMIT = 64


class WireError(ValueError):
    """A payload does not match the declared wire shape or exceeds its limits."""


def pack(value: object) -> bytes:
    """Encode a compact value while enforcing host byte, depth, and node budgets."""
    buffer = bytearray()
    nodes_left = NODE_LIMIT

    def put(data: bytes) -> None:
        """Append one field or raw payload without exceeding the byte budget."""
        if len(buffer) + len(data) > BYTE_LIMIT:
            raise WireError("component payload exceeds byte limit")
        buffer.extend(data)

    def header(size: int, fixed: int, fixed_max: int, wide: int, byte: int = 0) -> None:
        """Write the shortest header for a string, array, or map."""
        if size <= fixed_max:
            put(bytes([fixed + size]))
        elif byte and size <= 255:
            put(bytes([byte, size]))
        elif size <= 65535:
            put(bytes([wide]) + size.to_bytes(2, "big"))
        else:
            put(bytes([wide + 1]) + size.to_bytes(4, "big"))

    def write(item: object, depth: int) -> None:
        """Emit one value, charging nested values and map keys independently."""
        nonlocal nodes_left
        nodes_left -= 1
        if nodes_left < 0 or depth > DEPTH_LIMIT:
            raise WireError("component payload exceeds complexity limit")
        if item is None:
            put(b"\xc0")
        elif type(item) is bool:
            put(b"\xc3" if item else b"\xc2")
        elif type(item) is int:
            if -32 <= item <= 127:
                put(bytes([item & 255]))
                return
            signed = item < 0
            markers = (0xD0, 0xD1, 0xD2, 0xD3) if signed else (0xCC, 0xCD, 0xCE, 0xCF)
            for marker, size in zip(markers, (1, 2, 4, 8)):
                try:
                    raw = item.to_bytes(size, "big", signed=signed)
                except OverflowError:
                    continue
                put(bytes([marker]) + raw)
                return
            raise WireError("integer outside MessagePack range")
        elif type(item) is float and math.isfinite(item):
            put(b"\xcb" + struct.pack(">d", item))
        elif type(item) is str:
            if len(item) > BYTE_LIMIT:
                raise WireError("component payload exceeds byte limit")
            try:
                raw = item.encode("utf-8")
            except UnicodeError as error:
                raise WireError("invalid UTF-8 string") from error
            header(len(raw), 0xA0, 31, 0xDA, 0xD9)
            put(raw)
        elif type(item) is list:
            header(len(item), 0x90, 15, 0xDC)
            for child in item:
                write(child, depth + 1)
        elif type(item) is dict:
            header(len(item), 0x80, 15, 0xDE)
            for key, child in item.items():
                if type(key) is not str:
                    raise WireError("expected a string map key")
                write(key, depth)
                write(child, depth + 1)
        else:
            raise WireError("unsupported MessagePack value")

    write(value, 0)
    return bytes(buffer)


class _Reader:
    """Track the cursor and token budget for one bounded incoming payload."""

    def __init__(self, data: bytes):
        """Start an exact-length read of one bounded payload."""
        if type(data) is not bytes or len(data) > BYTE_LIMIT:
            raise WireError("invalid component payload bytes")
        self.data = memoryview(data)
        self.position = 0
        self.nodes_left = NODE_LIMIT

    def take(self, size: int) -> memoryview:
        """Consume exactly the requested bytes or reject the truncated payload."""
        end = self.position + size
        if end > len(self.data):
            raise WireError("truncated MessagePack value")
        raw = self.data[self.position:end]
        self.position = end
        return raw

    def integer(self, size: int, *, signed: bool = False) -> int:
        """Read a fixed-width integer in MessagePack byte order."""
        return int.from_bytes(self.take(size), "big", signed=signed)

    def string(self, size: int) -> str:
        """Read UTF-8 text, including map keys, without accepting invalid bytes."""
        try:
            return bytes(self.take(size)).decode("utf-8")
        except UnicodeError as error:
            raise WireError("invalid MessagePack UTF-8 string") from error

    def container(self, size: int, depth: int, *, mapping: bool) -> object:
        """Reject impossible container lengths before allocating collections."""
        entries = size * (2 if mapping else 1)
        if entries > min(self.nodes_left, len(self.data) - self.position):
            raise WireError("invalid MessagePack container length")
        if not mapping:
            return [self.read(depth + 1) for _ in range(size)]
        values = {}
        for _ in range(size):
            key = self.read(depth, string_only=True)
            if key in values:
                raise WireError("duplicate MessagePack map key")
            values[key] = self.read(depth + 1)
        return values

    def read(self, depth: int, *, string_only: bool = False) -> object:
        """Read a JSON-compatible value or, for map keys, exactly one string."""
        self.nodes_left -= 1
        if self.nodes_left < 0 or depth > DEPTH_LIMIT:
            raise WireError("component payload exceeds complexity limit")
        marker = self.integer(1)
        if 0xA0 <= marker <= 0xBF:
            return self.string(marker - 0xA0)
        if 0xD9 <= marker <= 0xDB:
            return self.string(self.integer(1 << (marker - 0xD9)))
        if string_only:
            raise WireError("expected a MessagePack string map key")
        if marker <= 0x7F:
            return marker
        if marker >= 0xE0:
            return marker - 256
        if 0x80 <= marker <= 0x9F:
            return self.container(marker & 15, depth, mapping=marker < 0x90)
        if marker == 0xC0:
            return None
        if marker in (0xC2, 0xC3):
            return marker == 0xC3
        if marker in (0xCA, 0xCB):
            fmt, width = (">f", 4) if marker == 0xCA else (">d", 8)
            value = struct.unpack(fmt, self.take(width))[0]
            if not math.isfinite(value):
                raise WireError("invalid MessagePack floating-point number")
            return value
        if 0xCC <= marker <= 0xD3:
            return self.integer(1 << ((marker - 0xCC) % 4), signed=marker >= 0xD0)
        if 0xDC <= marker <= 0xDF:
            return self.container(self.integer(2 << (marker & 1)), depth, mapping=marker >= 0xDE)
        raise WireError("unsupported MessagePack marker")


def unpack(data: bytes) -> object:
    """Decode exactly one bounded JSON-compatible compact value."""
    reader = _Reader(data)
    value = reader.read(0)
    if reader.position != len(data):
        raise WireError("trailing MessagePack bytes")
    return value
