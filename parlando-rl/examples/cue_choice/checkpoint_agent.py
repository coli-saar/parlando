"""Serve a trained cue-choice checkpoint as a Parlando SDK agent."""

import argparse
from functools import partial
from pathlib import Path

import torch
from parlando_agent_sdk import serve
from transformers import GPT2LMHeadModel, PreTrainedTokenizerFast

from train_grpo import CueChoiceAgent, greedy_choice


def main() -> None:
    """Load one checkpoint and serve it over the SDK's loopback gRPC endpoint."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "checkpoint", type=Path, help="directory containing the saved model and tokenizer"
    )
    parser.add_argument("--port", type=int, default=50061)
    args = parser.parse_args()

    torch.set_num_threads(1)
    model = GPT2LMHeadModel.from_pretrained(args.checkpoint, local_files_only=True).eval()
    tokenizer = PreTrainedTokenizerFast.from_pretrained(args.checkpoint, local_files_only=True)
    select = partial(greedy_choice, model, tokenizer)
    serve(lambda _context: CueChoiceAgent(select), port=args.port)


if __name__ == "__main__":
    main()
