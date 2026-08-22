#import "@preview/pergamon:0.7.1": *
#import "@preview/bananote:0.1.2": *
#import "@preview/ctheorems:1.1.3": *

#show: note.with(
  title: [A Cue--Choice Experiment for Reinforcement Learning with Qwen in Parlando],
  authors: (
    ([Alexander Koller], [Saarland University]),
  ),
  date: datetime.today(),
  version: [0.1],
)

#show: thmrules.with(qed-symbol: $square$)

#let proposition = thmbox(
  "proposition",
  "Proposition",
  base: none,
  fill: rgb("#fbfaf4"),
  stroke: rgb("#d9cfa8"),
  inset: (x: 1em, y: 0.75em),
)

#let query-example(body) = block(
  width: 100%,
  fill: rgb("#f7fbff"),
  stroke: rgb("#b7cde2"),
  inset: 0.8em,
)[#body]

#let clip = math.op("clip")

#abstract[
We present a small reinforcement-learning experiment designed to test the agent--agent training infrastructure of Parlando. A scripted dealer exposes one of four nonsense cues, and a Qwen2.5-0.5B policy must select the associated action. The task is deliberately a contextual bandit rather than a language benchmark: it isolates session execution, role-safe observations, reward assignment, trajectory transport, checkpointed policy updates, and held-out validation. The learner combines a frozen Qwen backbone, rank-8 low-rank adaptation, a four-way policy head, a scalar value head, and clipped proximal policy optimization. Across 768 training and 192 validation sessions, every planned session completed and six checkpoints were produced. Held-out accuracy rose from 28.1% at the first checkpoint to 93.8% and then 100% at checkpoints three and four, before falling to 65.6% and recovering to 84.4%. The experiment therefore verifies the complete training path and demonstrates learnability, while the non-monotonic curve shows that this compact PPO configuration is not yet a stable optimization recipe.
]

= Introduction

An agent--agent reinforcement-learning system has two distinct correctness obligations. It must execute the learning algorithm correctly, but it must also preserve the semantics of the environment while observations, actions, rewards, and checkpoints cross process and language boundaries. A failure in either layer can produce an apparently plausible training run. For example, a learner may optimize the wrong player's observation, reuse a mutable checkpoint during validation, or receive a reward that was computed from non-authoritative state.

The cue--choice experiment separates these obligations by making the learning problem small and inspectable. Player B is a scripted dealer. It reveals one of four cues to player A, which must choose one of four tree names. The mapping is fixed but absent from the learner's observation: _dax_ maps to _oak_, _wug_ to _pine_, _kiki_ to _birch_, and _zorp_ to _elm_. A correct choice receives reward $+1$ and an incorrect choice receives $-1$. Each session therefore contains exactly one reward-bearing learner transition.

This design is not intended to establish a new reinforcement-learning result. Its purpose is methodological: a real pretrained language model and a real policy-gradient update exercise the infrastructure without allowing long-horizon credit assignment or free-form generation errors to obscure the transport and scheduling questions. The experiment asks two bounded questions. First, can Parlando complete a checkpointed train--validate loop while retaining typed game semantics and role safety? Second, does the resulting trajectory stream contain enough information to train a Qwen-backed policy above the 25% chance baseline?

The recorded run answers both questions positively, with an important qualification. All 960 sessions completed, six immutable checkpoint identifiers were recorded, and validation reached 100% at one checkpoint. However, accuracy later declined. The result supports the infrastructure claim and the basic learnability claim, but not a claim of stable convergence.

= Task and experimental design

== A two-action session with one learner decision

The game has three authoritative phases:

```text
AwaitingDeal  -- Deal by B -->  Choosing  -- Choose by A -->  Complete
```

The first action changes the environment but carries zero reward. The second action ends the session and determines correctness. Actions submitted by the wrong role or in the wrong phase are rejected by the Rust game. Player A receives a role-safe observation containing the visible cue and a seed-derived trial nonce, together with the four legal typed actions. It does not receive the correct answer or the dealer's internal state.

The nonce prevents exact serialized observations from repeating across the training and validation sets. Training uses seeds 1 through 16 for each cue; validation uses seeds 1001 through 1008. Both sets contain the same four cue categories, so validation measures generalization across irrelevant nonce values rather than generalization to unseen cues.

#query-example[
For the observation

```json
{"view":"cue", "cue":"wug", "trial_nonce":"trial-00000000000003e9"}
```

the policy receives four typed choices. The correct action is `{"type":"choose", "choice":"pine"}`. The nonce changes between sessions but has no effect on the correct choice.
]

== Schedule, data, and checkpoints

The activation schedule begins with player B and alternates after every response. Because both responses are actions and the learner's action terminates the game, a session always contains two decisions and two accepted actions. No message or yield behavior is required.

One training epoch is a deterministic sweep over 64 sessions: four cues times 16 seeds. The configuration runs 12 epochs and updates the learner after every two epochs. Thus each update consumes 128 terminal learner transitions, and the experiment produces six checkpoints. After each checkpoint, validation runs 32 held-out sessions: four cues times eight seeds. Overall, the run contains 768 training sessions and 192 validation sessions.

The initial policy is uniform because the policy and value heads begin with zero weights and biases. Chance accuracy is therefore 25% before learning, independent of arbitrary head initialization. The learner saves every third checkpoint to disk, while Parlando records every checkpoint identifier required to describe the run.

= Learning method

== Model and action representation

The policy uses the instruction-tuned Qwen2.5 model with approximately 0.5 billion parameters #cite("qwen25"). The learner converts the role-safe observation to canonical JSON inside a short text prompt. Qwen encodes this prompt, and the final token representation feeds two trainable heads: a four-way categorical policy and a scalar value estimator.

The fixed action head narrows the interpretation of the experiment. It does not ask Qwen to generate JSON or natural language. Instead, each logit corresponds to one semantic choice, and the selected choice is converted back to the matching typed Rust action. This removes parsing failures and makes policy accuracy directly measurable. It also means that the experiment tests representation learning and policy updating, not open-ended language generation.

The pretrained backbone is adapted with low-rank matrices on the attention query and value projections. Low-rank adaptation freezes the original model parameters and learns a smaller set of injected matrices #cite("lora"). We use rank 8, together with the policy and value heads. This reduces the trainable state that must be copied into each immutable checkpoint.

== PPO update

The learner applies clipped proximal policy optimization (PPO), which permits several minibatch passes over one on-policy batch while limiting large policy-ratio changes #cite("ppo"). For a selected action $a_t$ under observation $o_t$, let

$ r_t(theta) = pi_theta(a_t | o_t) / pi_(theta_"old")(a_t | o_t) $

denote the probability ratio and $hat(A)_t$ the estimated advantage. The policy term is

$ L_"clip"(theta) = - E_t [min(r_t(theta) hat(A)_t, clip(r_t(theta), 1-epsilon, 1+epsilon) hat(A)_t)]. $

The implementation adds a squared value loss and an entropy bonus. It uses $epsilon=0.2$, entropy coefficient 0.01, value coefficient 0.5, learning rate $10^(-4)$, four PPO epochs, and minibatches of 32 transitions. These values are smoke-test settings rather than tuned hyperparameters.

Every learner episode has one terminal decision. The return is consequently the observed reward, and the next-state value is zero. Generalized advantage estimation is still computed through TorchRL, but the one-step structure avoids an ambiguous temporal credit assignment. The learner recomputes old log probabilities and values from the immutable base checkpoint because the Parlando trajectory does not carry rollout-time logits.

= Implementation architecture

== Separation of environment and learner

Parlando retains authority over game state. The Rust game validates actions, computes role-specific observations, detects completion, and invokes a separately registered reward function on authoritative pre- and post-action states. The learner receives only serialized observations, legal actions, selected actions, and numeric rewards. It cannot inspect or modify the authoritative state.

This division also separates game semantics from optimization semantics. The game knows that _wug_ maps to _pine_, but it does not know how the learner encodes _wug_. Conversely, the Python learner defines the prompt, Qwen representation, policy distribution, and update rule, but it cannot decide whether an action was applicable or what reward it earned.

== One generic remote boundary

The Rust runner registers a non-generic `RemoteAgent`. Rust supplies a game-specific trait implementation when the factory is used, serializes typed observations and actions to JSON-shaped protocol values, and deserializes returned actions into the game's concrete action type. The transport object itself stores no game type and contains no cue--choice logic.

Model configuration is nested under an opaque `config` field in the experiment YAML. Parlando interprets transport fields such as the endpoint and agent identity, but forwards model name, device, temperature, LoRA rank, and optimizer settings unchanged to Python. With the configured `auto` device, the learner prefers CUDA, then Apple MPS, and finally CPU. A checkpoint identifier is a separate protocol field rather than an entry injected into model configuration. This distinction matters because checkpoint provenance belongs to experiment execution, whereas model configuration belongs to the remote implementation.

The Python process exposes two services on one loopback endpoint. The ordinary agent service creates a lightweight session-local inference object that retains its checkpoint, seed, and latest observation. The learner service receives a batch and returns a new checkpoint identifier. Both protocol adapters live in the reusable Python SDK; cue--choice code implements only the inference and training callbacks.

== Checkpoint consistency and replay

The Python process preloads Qwen once. Session-local agents share the model runtime rather than loading one copy per concurrent session. A lock serializes inference and updates against the mutable in-process model representation. Published checkpoints are immutable snapshots of the trainable state.

Each training request contains a run-scoped update identifier. The learner hashes the base checkpoint, trajectory batch, completed epoch count, and settings. Repeating an identical update returns the prior checkpoint; reusing the identifier for different content fails. On the Rust side, finalized session results and checkpoint records allow a restarted coordinator to skip completed work. These two mechanisms cover different failure windows: runner records prevent ordinary replay, while learner idempotency covers a failure after a remote update succeeds but before its checkpoint record is finalized.

= Results

All 768 planned training sessions and all 192 planned validation sessions completed. No session was marked failed, and each of the six planned updates produced a distinct checkpoint record. The infrastructure outcome was therefore 960 completed sessions out of 960 planned sessions.

#figure(
  table(
    columns: (0.8fr, 1.5fr, 1fr, 1fr),
    align: (center, left, right, right),
    table.header([*Checkpoint*], [*Policy identifier*], [*Correct / 32*], [*Accuracy*]),
    [1], [`update-000001`], [9], [28.1%],
    [2], [`update-000002`], [12], [37.5%],
    [3], [`update-000003`], [30], [93.8%],
    [4], [`update-000004`], [32], [100.0%],
    [5], [`update-000005`], [21], [65.6%],
    [6], [`update-000006`], [27], [84.4%],
  ),
  caption: [Held-out validation accuracy at each learner checkpoint. Each row contains eight nonce-disjoint trials for each of four cues.],
) <validation-results>

The first checkpoint remains close to the 25% baseline. Accuracy then rises to 37.5%, 93.8%, and 100% over the next three checkpoints. This establishes that the role-safe observation, typed selected action, and signed reward arrive at the learner in a usable relation: a learner receiving shuffled rewards or the dealer's observation would not be expected to recover the cue mapping so directly.

The later decline is equally informative. Accuracy falls to 65.6% at checkpoint five and recovers only partly, to 84.4%, at checkpoint six. Because every checkpoint is evaluated on the same balanced validation coordinates and because validation seeds differ from training seeds, the change reflects policy instability rather than a changing evaluation mixture. The present evidence does not distinguish excessive learning rate, repeated minibatch optimization, entropy pressure, value-loss interaction, or sampling variance in training trajectories. It therefore licenses a diagnosis of optimization instability, but not a specific causal explanation.

Training accuracy follows the same broad pattern. The policy scores 25.0% in the first two epochs, 26.6% in epochs three and four, 37.5% in epochs five and six, 76.6% in epochs seven and eight, and 96.9% in epochs nine and ten. Under checkpoint five it falls to 59.4% in epochs eleven and twelve. The matching decline on training and validation data argues against ordinary held-out overfitting as the sole explanation.

#proposition("Bounded empirical conclusion")[
The run demonstrates end-to-end execution and above-chance learning for this four-cue contextual bandit. It does not demonstrate monotonic improvement, stable PPO convergence, long-horizon credit assignment, or dialogue generation.
]

= Discussion

The experiment succeeds as an infrastructure test because the task is narrower than the system being tested. A two-decision game makes each recorded artifact auditable: one can trace the dealer action, learner observation, legal action set, chosen typed action, authoritative completion, role reward, update identifier, and validation checkpoint. At the same time, Qwen, LoRA, and PPO ensure that success cannot be attributed to an in-process table lookup hidden inside the runner.

The result also supports the architectural separation between the session runner and the learner. Parlando can remain game-typed in Rust without requiring experiment-specific Rust transport code. The Python learner receives generic JSON-shaped values but retains responsibility for their model-specific interpretation. This arrangement permits a single remote implementation to serve arbitrary serializable games while keeping action applicability and rewards on the authoritative side of the boundary.

Several limitations constrain the scientific interpretation. The fixed policy head bypasses generation and parsing. The cue mapping is shared between training and validation. Each episode has one learner action, so no delayed reward is present. Only one training run is reported, and the validation set contains 32 trials per checkpoint; no confidence intervals across random training seeds are available. The in-memory checkpoint store also assumes that the Python process survives for the active run, except at the configured disk-save cadence.

A natural follow-up experiment would repeat training under several optimizer seeds and compare lower learning rates or fewer PPO epochs. A second extension should introduce a multi-step communication game in which messages affect later action rewards. That extension requires an explicit definition of which communication event forms an environment step; it should not be inferred merely by adding messages to the present trajectory format.

= Conclusion

The cue--choice experiment provides a compact scientific test of Parlando's agent--agent RL path. A Qwen2.5-0.5B policy trained through LoRA and PPO completed six train--validate cycles over 960 sessions without an infrastructure failure. The model learned the hidden four-way mapping and reached perfect validation accuracy at one checkpoint. Its subsequent regression shows that the current hyperparameters are suitable for exercising the system but not for claiming stable convergence. The main result is therefore a verified implementation boundary: typed game execution and authoritative reward computation can remain in Rust while a generic remote Python learner owns observation encoding, policy optimization, and checkpoint storage.

#add-bib-resource(read("references.bib"))
#print-bananote-bibliography()
