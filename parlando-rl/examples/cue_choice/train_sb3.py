"""Train stock Stable-Baselines3 PPO on Cue Choice and serve it as an SDK agent."""

import argparse
from pathlib import Path
from typing import Any

import numpy as np
import torch
from gymnasium import spaces
from parlando_agent_sdk import Reason, serve
from stable_baselines3 import PPO

from parlando_rl import ParlandoAction, ParlandoEpisode, ParlandoObservation
from parlando_rl.gymnasium import GymPolicyAgent, ParlandoGymEnv

from train import LEVELS, correctness

CHOICES = ("birch", "elm", "oak", "pine")
DIST = Path(__file__).resolve().parents[3] / "experiments" / "cue-choice-rl" / "dist"


def encode_observation(frame: ParlandoObservation) -> np.ndarray:
    """One-hot encode the learner's cue and represent other phases with zeros."""
    result = np.zeros(len(LEVELS), dtype=np.float32)
    if "cue" in frame.view:
        result[LEVELS.index(frame.view["cue"])] = 1.0
    return result


def decode_action(action: int, view: dict[str, Any]) -> ParlandoAction:
    """Resolve a stable choice index to the matching offered game action."""
    choice = CHOICES[int(action)]
    return ParlandoAction(
        action=next(item for item in view["available_actions"] if item["choice"] == choice)
    )


def sample_config(rng: np.random.Generator) -> dict[str, str]:
    """Sample a cue level from the seeded Gymnasium RNG."""
    return {"level": LEVELS[int(rng.integers(len(LEVELS)))]}


def has_choice(view: dict[str, Any], reason: Reason) -> bool:
    """Yield on hosted callbacks before the dealer makes a cue available."""
    return bool(view.get("available_actions"))


def score(model: PPO, env: ParlandoGymEnv) -> float:
    """Evaluate the greedy policy once for each cue on separate replay seeds."""
    rewards = []
    for index, level in enumerate(LEVELS):
        obs, _ = env.reset(seed=10001 + index, options={"config": {"level": level}})
        action, _ = model.predict(obs, deterministic=True)
        _, reward, terminated, truncated, _ = env.step(action)
        if not terminated or truncated:
            raise RuntimeError("Cue Choice evaluation did not complete")
        rewards.append(reward)
    return sum(rewards) / len(rewards)


def train(game: Path, dealer: Path, checkpoint: Path, timesteps: int) -> None:
    """Train and save a PPO checkpoint from local packaged WASM episodes."""
    with ParlandoGymEnv(
        ParlandoEpisode(
            game=game,
            opponent={"factory": "wasm", "path": dealer},
            learner_role="A",
            schedule={"kind": "alternate_after_action", "first": "player_b"},
            reward=correctness,
        ),
        observation_space=spaces.Box(0.0, 1.0, shape=(len(LEVELS),), dtype=np.float32),
        action_space=spaces.Discrete(len(CHOICES)),
        encode_observation=encode_observation,
        decode_action=decode_action,
        sample_config=sample_config,
    ) as env:
        model = PPO(
            "MlpPolicy", env, device="cpu", seed=1, n_steps=128, batch_size=64,
            learning_rate=0.003,
        )
        before = score(model, env)
        model.learn(total_timesteps=timesteps)
        after = score(model, env)
        model.save(checkpoint)
    print(f"Greedy accuracy: {before:.0%} before training, {after:.0%} after training")


def serve_checkpoint(checkpoint: Path, port: int) -> None:
    """Load the checkpoint once and create an SDK agent for each served session."""
    model = PPO.load(checkpoint, device="cpu")

    def factory(_context: Any) -> GymPolicyAgent:
        """Give each hosted session its own view and model prediction state."""
        return GymPolicyAgent(
            model,
            action_space=model.action_space,
            encode_observation=encode_observation,
            decode_action=decode_action,
            should_act=has_choice,
        )

    serve(factory, port=port)


def main() -> None:
    """Train a checkpoint or serve an existing one on loopback gRPC."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    training = commands.add_parser("train")
    training.add_argument("--game", type=Path, default=DIST / "cue-choice-0.1.0.zip")
    training.add_argument("--dealer", type=Path, default=DIST / "agent-cue_choice.dealer-1.0.0.zip")
    training.add_argument("--checkpoint", type=Path, default=Path("cue-choice-ppo.zip"))
    training.add_argument("--timesteps", type=int, default=2048)
    serving = commands.add_parser("serve")
    serving.add_argument("checkpoint", type=Path)
    serving.add_argument("--port", type=int, default=50061)
    args = parser.parse_args()
    torch.set_num_threads(1)
    if args.command == "train":
        train(args.game, args.dealer, args.checkpoint, args.timesteps)
    else:
        serve_checkpoint(args.checkpoint, args.port)


if __name__ == "__main__":
    main()
