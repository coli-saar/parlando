"""Expose the packaged Parlando app at OpenEnv's default entry point."""

from parlando_rl.openenv.server import app, main as run_server

__all__ = ["app", "main"]


def main() -> None:
    """Run the configured OpenEnv server."""
    run_server()


if __name__ == "__main__":
    main()
