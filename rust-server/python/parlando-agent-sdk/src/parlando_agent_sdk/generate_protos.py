"""Generates Python gRPC modules for the Parlando remote-agent protocol."""

from __future__ import annotations

from pathlib import Path

import grpc_tools
from grpc_tools import protoc


def main() -> int:
    """Runs grpc_tools.protoc for the bundled Parlando agent protobuf."""
    package_dir = Path(__file__).resolve().parent
    proto_dir = package_dir / "protos"
    generated_dir = package_dir / "generated"
    generated_dir.mkdir(exist_ok=True)
    init_file = generated_dir / "__init__.py"
    init_file.touch()
    proto_files = [
        proto_dir / "parlando_agent_v3.proto",
        proto_dir / "parlando_rl_v1.proto",
    ]
    bundled_proto_dir = Path(grpc_tools.__file__).resolve().parent / "_proto"
    return protoc.main(
        [
            "grpc_tools.protoc",
            f"--proto_path={proto_dir}",
            f"--proto_path={bundled_proto_dir}",
            f"--python_out={generated_dir}",
            f"--grpc_python_out={generated_dir}",
            *(str(path) for path in proto_files),
        ]
    )


if __name__ == "__main__":
    raise SystemExit(main())
