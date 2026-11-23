"""Compile Python annotations into a compact codec and its public schema."""

from copy import deepcopy
from dataclasses import InitVar, dataclass, fields, is_dataclass
import math
from types import NoneType, UnionType
from typing import Annotated, Callable, Generic, Tuple, TypeVar, Union, get_args, get_origin, get_type_hints

from ._messagepack import DEPTH_LIMIT, NODE_LIMIT, WireError, pack, unpack

U8 = Annotated[int, "parlando.u8"]
"""An integer from 0 to 255, declared publicly as an unsigned integer."""

U64 = Annotated[int, "parlando.u64"]
"""An integer from 0 to 2**64 - 1, declared publicly as an unsigned integer."""

_UNSIGNED_LIMITS = {"parlando.u8": 255, "parlando.u64": (1 << 64) - 1}
_SCALARS = {NoneType: "null", bool: "bool", int: "i64", float: "f64", str: "string"}
_T = TypeVar("_T")
_Class = TypeVar("_Class", bound=type)


def variant(name: str, *, tag: str = "type") -> Callable[[_Class], _Class]:
    """Mark a dataclass as a named alternative in an internally tagged union.

    All alternatives must use the same tag property. Union order is the
    compact wire order. A dataclass with no fields is a unit alternative.
    A decorated class can also be used as a one-alternative union.
    """
    if not name or not tag or type(name) is not str or type(tag) is not str:
        raise TypeError("variant name and tag must be nonempty strings")

    def decorate(cls: _Class) -> _Class:
        """Attach the public name and tag property to this alternative."""
        setattr(cls, "__parlando_variant__", (name, tag))
        return cls

    return decorate


@dataclass(frozen=True)
class _Field:
    """A dataclass constructor argument and its compact field codec."""

    attribute: str
    shape: "_Shape"

    def schema(self) -> dict:
        """Expose the attribute name as the public property name."""
        return {"name": self.attribute, "schema": self.shape.schema}


@dataclass(frozen=True)
class _Alternative:
    """A tagged dataclass and its ordered fields."""

    annotation: type
    name: str
    tag: str
    fields: tuple[_Field, ...]

    def schema(self) -> dict:
        """Describe a unit or named variant without its separate discriminator."""
        payload = (
            {"type": "struct", "value": [field.schema() for field in self.fields]}
            if self.fields else {"type": "unit"}
        )
        return {"name": self.name, "payload": payload}


@dataclass
class _Conversion:
    """Bound one conversion so partly encoded collections cannot grow without limit."""

    decoding: bool
    remaining: int = NODE_LIMIT

    def charge(self) -> None:
        """Charge one typed value against the common payload node budget."""
        self.remaining -= 1
        if self.remaining < 0:
            raise WireError("component payload exceeds complexity limit")


@dataclass(frozen=True)
class _Shape:
    """A compiled schema and the information required to convert typed values."""

    schema: dict
    children: tuple["_Shape", ...] = ()
    record: type | None = None
    fields: tuple[_Field, ...] = ()
    alternatives: tuple[_Alternative, ...] = ()
    all_unit: bool = False
    tuple_list: bool = False
    maximum: int | None = None

    def convert(self, value, conversion: _Conversion):
        """Check the declared shape while converting between Python and compact values."""
        kind = self.schema["type"]
        if kind == "option":
            if value is not None:
                return self.children[0].convert(value, conversion)
            conversion.charge()
            return None
        conversion.charge()
        decoding = conversion.decoding
        if kind in ("list", "tuple"):
            value_type = tuple if not decoding and (kind == "tuple" or self.tuple_list) else list
            if type(value) is not value_type or len(value) > NODE_LIMIT:
                raise WireError(f"invalid compact {kind}")
            if kind == "tuple":
                if len(value) != len(self.children):
                    raise WireError("invalid compact tuple length")
                items = [shape.convert(item, conversion) for shape, item in zip(self.children, value)]
            else:
                items = [self.children[0].convert(item, conversion) for item in value]
            return tuple(items) if decoding and (kind == "tuple" or self.tuple_list) else items
        if kind == "map":
            if type(value) is not dict or len(value) > NODE_LIMIT // 2:
                raise WireError("invalid compact map")
            if any(type(key) is not str for key in value):
                raise WireError("expected a string map key")
            return {key: self.children[0].convert(item, conversion) for key, item in value.items()}
        if kind == "struct":
            if decoding:
                return self.record(**_decode_fields(self.fields, value, conversion))
            return _encode_fields(self.record, self.fields, value, conversion)
        if kind == "enum":
            return self._enum(value, conversion)
        self._check_scalar(kind, value)
        return value

    def _enum(self, value, conversion: _Conversion):
        """Convert a tagged dataclass to an ordinal and flattened payload or back."""
        if conversion.decoding:
            if self.all_unit:
                ordinal, items = value, []
            elif type(value) is list and value:
                ordinal, *items = value
            else:
                raise WireError("expected a compact enum array")
            if type(ordinal) is not int or not 0 <= ordinal < len(self.alternatives):
                raise WireError("invalid compact enum ordinal")
            variant = self.alternatives[ordinal]
            return variant.annotation(**_decode_fields(variant.fields, items, conversion))
        for ordinal, variant in enumerate(self.alternatives):
            if type(value) is variant.annotation:
                if self.all_unit:
                    return ordinal
                return [ordinal, *_encode_fields(variant.annotation, variant.fields, value, conversion)]
        raise WireError("invalid Python enum alternative")

    def _check_scalar(self, kind: str, value) -> None:
        """Require the exact scalar type and reject overflow or nonfinite floats."""
        if kind == "i64":
            valid = type(value) is int and -(1 << 63) <= value < (1 << 63)
        elif kind == "u64":
            valid = type(value) is int and 0 <= value <= self.maximum
        elif kind == "f64":
            valid = type(value) is float and math.isfinite(value)
        else:
            valid = type(value) is {"bool": bool, "string": str, "null": NoneType}[kind]
        if not valid:
            raise WireError(f"invalid compact {kind}")


def _encode_fields(record: type, spec: tuple[_Field, ...], value, conversion: _Conversion) -> list:
    """Emit fixed-position values from the exact declared dataclass type."""
    if type(value) is not record:
        raise WireError(f"expected {record.__name__}")
    return [field.shape.convert(getattr(value, field.attribute), conversion) for field in spec]


def _decode_fields(spec: tuple[_Field, ...], value, conversion: _Conversion) -> dict:
    """Convert exact-length positional fields into dataclass constructor arguments."""
    if type(value) is not list or len(value) != len(spec):
        raise WireError("invalid compact field count")
    return {
        field.attribute: field.shape.convert(item, conversion)
        for field, item in zip(spec, value)
    }


class _Compiler:
    """Compile one nonrecursive annotation tree against the host schema budgets."""

    def __init__(self):
        """Start a budget shared across the schemas this compiler builds."""
        self.remaining = NODE_LIMIT

    def charge(self, depth: int) -> None:
        """Count a schema node or a variant declaration."""
        self.remaining -= 1
        if self.remaining < 0 or depth > DEPTH_LIMIT:
            raise TypeError("component schema exceeds complexity limit")

    def shape(self, annotation, depth: int = 0, ancestors: tuple = ()) -> _Shape:
        """Translate a supported annotation into the sole source of codec and schema."""
        self.charge(depth)
        if annotation is None:
            annotation = NoneType
        origin, args = get_origin(annotation), get_args(annotation)
        if annotation in _SCALARS:
            return _Shape({"type": _SCALARS[annotation]})
        if origin is Annotated:
            if len(args) == 2 and args[0] is int and args[1] in _UNSIGNED_LIMITS:
                return _Shape({"type": "u64"}, maximum=_UNSIGNED_LIMITS[args[1]])
        if origin in (UnionType, Union):
            nonnull = tuple(arg for arg in args if arg is not NoneType)
            if len(nonnull) < len(args):
                inner = self.shape(Union[nonnull], depth + 1, ancestors)
                return _Shape({"type": "option", "value": inner.schema}, (inner,))
            return self.enum(args, depth, ancestors)
        if origin is list and len(args) == 1:
            child = self.shape(args[0], depth + 1, ancestors)
            return _Shape({"type": "list", "value": child.schema}, (child,))
        if origin is dict and len(args) == 2 and args[0] is str:
            child = self.shape(args[1], depth + 1, ancestors)
            return _Shape({"type": "map", "value": child.schema}, (child,))
        if origin is tuple and annotation is not Tuple:
            if len(args) == 2 and args[1] is Ellipsis:
                child = self.shape(args[0], depth + 1, ancestors)
                return _Shape({"type": "list", "value": child.schema}, (child,), tuple_list=True)
            children = tuple(self.shape(item, depth + 1, ancestors) for item in args)
            return _Shape({"type": "tuple", "value": [item.schema for item in children]}, children)
        if isinstance(annotation, type) and is_dataclass(annotation):
            if "__parlando_variant__" in vars(annotation):
                return self.enum((annotation,), depth, ancestors)
            spec = self.record_fields(annotation, depth, ancestors)
            return _Shape({"type": "struct", "value": [field.schema() for field in spec]},
                          record=annotation, fields=spec)
        raise TypeError(f"unsupported component annotation {annotation!r}")

    def record_fields(self, annotation: type, depth: int, ancestors: tuple) -> tuple[_Field, ...]:
        """Use the dataclass's declaration order and reject recursive or non-init fields."""
        if annotation in ancestors:
            raise TypeError(f"recursive component annotation {annotation.__name__}")
        hints = get_type_hints(annotation, include_extras=True)
        if any(isinstance(hint, InitVar) for hint in hints.values()):
            raise TypeError(f"{annotation.__name__} constructor cannot use InitVar fields")
        spec = []
        for field in fields(annotation):
            if not field.init:
                raise TypeError(f"{annotation.__name__}.{field.name} must be a constructor field")
            spec.append(_Field(field.name, self.shape(hints[field.name], depth + 1, (*ancestors, annotation))))
        return tuple(spec)

    def enum(self, annotations: tuple, depth: int, ancestors: tuple) -> _Shape:
        """Compile a union of distinctly named dataclasses sharing one public tag."""
        alternatives = []
        for annotation in annotations:
            self.charge(depth)
            if not isinstance(annotation, type) or not is_dataclass(annotation):
                raise TypeError("component union alternatives must be @variant dataclasses")
            marker = vars(annotation).get("__parlando_variant__")
            if marker is None:
                raise TypeError("component union alternatives must be @variant dataclasses")
            name, tag = marker
            spec = self.record_fields(annotation, depth, ancestors)
            if any(field.attribute == tag for field in spec):
                raise TypeError(f"variant payload uses reserved tag property {tag!r}")
            alternatives.append(_Alternative(annotation, name, tag, spec))
        if len({variant.name for variant in alternatives}) != len(alternatives):
            raise TypeError("component union has duplicate variant names")
        if len({variant.tag for variant in alternatives}) != 1:
            raise TypeError("component union alternatives must share the same tag property")
        return _Shape(
            {"type": "enum", "value": {
                "representation": {"type": "internal", "value": {"tag": alternatives[0].tag}},
                "variants": [variant.schema() for variant in alternatives],
            }},
            alternatives=tuple(alternatives),
            all_unit=all(not variant.fields for variant in alternatives),
        )


class Codec(Generic[_T]):
    """Read and write compact MessagePack for a supported Python annotation.

    Supports None, bool, int (signed 64-bit), U8, U64, finite float, str,
    homogeneous lists and string-keyed dicts, fixed and homogeneous tuples,
    dataclasses, optional values, and internally tagged dataclass unions.
    Dataclass field order defines wire slot order.
    """

    def __init__(self, annotation: type[_T], *, _compiler: _Compiler | None = None):
        """Compile one wire declaration from its Python annotation."""
        self._shape = (_compiler if _compiler is not None else _Compiler()).shape(annotation)

    def schema(self) -> dict:
        """Return the corresponding public Parlando schema as a fresh JSON object."""
        return deepcopy(self._shape.schema)

    def encode(self, value: _T) -> bytes:
        """Encode a typed value, raising WireError on an invalid payload."""
        return pack(self._shape.convert(value, _Conversion(decoding=False)))

    def decode(self, payload: bytes) -> _T:
        """Decode a complete payload into the declared Python type."""
        return self._shape.convert(unpack(payload), _Conversion(decoding=True))
