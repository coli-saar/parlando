# Parlando RL

Train a policy in a Parlando game and use the trained policy as an ordinary Parlando agent. One Python wheel includes the native Parlando runtime. Each episode gives the learner one seat and runs the opponent until the learner can act again.

Install the adapter you use. From the repository root, for example, install Gymnasium and Stable-Baselines3 and build the cue-choice game and dealer.

```sh
python -m pip install ./parlando-agent-sdk './parlando-rl[sb3]'
./experiments/cue-choice-rl/build.sh
python parlando-rl/examples/cue_choice/train_sb3.py --help
```

| Extra | Use |
| --- | --- |
| `gymnasium` | Present a game through fixed observation and action spaces. The `sb3` extra also installs Stable-Baselines3. |
| `verifiers` | Present a game as a chat task, collect model decisions and score them using the game's rewards. Requires Python 3.11 or newer. |
| `openenv` | Use OpenEnv in process or serve the game over a persistent WebSocket. |

A game can implement just the representation its users need. Gymnasium and Verifiers can run the native game locally without an OpenEnv server. For the episode and policy interfaces, see [training with Parlando RL](../docs/training-with-parlando-rl.md). For remote clients, see [serving a game with OpenEnv](../docs/training-with-openenv.md).
