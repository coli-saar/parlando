"""Train an epsilon-greedy bandit against the cue-choice WASM dealer."""

import argparse
import asyncio
import json
import random
from collections import defaultdict
from pathlib import Path

from parlando_rl import ParlandoAction, ParlandoEpisode, RewardContext

LEVELS = ("dax", "wug", "kiki", "zorp")


def correctness(frame: RewardContext) -> float:
    """Award one point for a correct terminal choice and zero otherwise."""
    return float(frame.completion is not None and frame.completion["correct"])


async def train(game: Path, dealer: Path, episodes: int) -> tuple[dict[str, str], float]:
    """Estimate each cue-action value from deterministic schedules and seeded exploration."""
    env = ParlandoEpisode(
        game=game,
        opponent={"factory": "wasm", "path": dealer},
        learner_role="A",
        schedule={"kind": "alternate_after_action", "first": "player_b"},
        reward=correctness,
    )
    values: dict[str, dict[str, float]] = defaultdict(dict)
    counts: dict[tuple[str, str], int] = defaultdict(int)
    rng = random.Random(0)
    try:
        for seed in range(episodes):
            obs = await env.reset_async(seed=seed, config={"level": LEVELS[seed % len(LEVELS)]})
            cue = obs.view["cue"]
            actions = obs.view["available_actions"]
            for action in actions:
                values[cue].setdefault(action["choice"], 0.0)
            action = (
                rng.choice(actions)
                if rng.random() < 0.1
                else max(actions, key=lambda action: values[cue][action["choice"]])
            )
            choice = action["choice"]
            obs = await env.step_async(ParlandoAction(action=action))
            if not obs.done or obs.metadata.get("completion") is None:
                raise RuntimeError(f"cue-choice episode {seed} did not complete: {obs.metadata}")
            counts[cue, choice] += 1
            values[cue][choice] += (obs.reward - values[cue][choice]) / counts[cue, choice]
    finally:
        await env.close_async()
    table = {cue: max(choices, key=choices.__getitem__) for cue, choices in values.items()}
    estimate = sum(values[cue][choice] for cue, choice in table.items()) / len(table)
    return table, estimate


def main() -> None:
    """Write the learned checkpoint and report its estimated mean reward."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game", type=Path, required=True)
    parser.add_argument("--dealer", type=Path, required=True)
    parser.add_argument("--episodes", type=int, default=5000)
    parser.add_argument("--table", type=Path, default=Path("table.json"))
    args = parser.parse_args()
    if not len(LEVELS) <= args.episodes <= 10000:
        parser.error("--episodes must be between 4 and 10000 to cover all cues and reserve held-out seeds")
    table, estimate = asyncio.run(train(args.game, args.dealer, args.episodes))
    args.table.write_text(json.dumps(table, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote {args.table}. Estimated mean reward: {estimate:.3f}")


if __name__ == "__main__":
    main()
