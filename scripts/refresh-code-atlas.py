"""Bundle current Graft symbol spans for the offline maintainer atlas."""

import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ATLAS = ROOT / "developer/code-atlas"
FILES = {
    "rust-server/src/app/connections.rs": "register unregister",
    "rust-server/src/app/game_socket.rs": "game_socket websocket_loop",
    "rust-server/src/app/audio_socket.rs": "audio_websocket_loop",
    "rust-server/src/app.rs": "AppState",
    "rust-server/src/app/admission.rs": "admit_session",
    "rust-server/src/app/participant_state.rs": "get_participant_state existing_assignment participant_state_response",
    "rust-server/src/app/readiness.rs": "maybe_start_game",
    "rust-server/src/app/client_message.rs": "handle_client_message",
    "rust-server/src/app/live_agent.rs": "start_agent_task handle_agent_response",
    "rust-server/src/app/conversation.rs": "commit_conversation_message commit_final_transcript",
    "rust-server/src/app/admin/replay.rs": "admin_session_replay",
    "rust-server/src/app/admin/data.rs": "admin_export",
    "rust-server/src/app/maintenance.rs": "cleanup_transient_rooms finalize_interrupted_sessions",
    "rust-server/src/app/routing.rs": "GameHost",
    "rust-server/src/app/routing/mounts.rs": "experiment_router participant_owner",
    "rust-server/src/app/session.rs": "LiveSession SessionLifecycle",
    "rust-server/src/app/lifecycle.rs": "allows_intake can_transition_to",
    "rust-server/src/app/session_transitions.rs": "expire_if_due_locked reserve_agent_delivery expire_live_session_locked depart_session_locked commit_end_locked",
    "rust-server/src/app/action.rs": "prepare_action submit_action",
    "rust-server/src/app/deadlines.rs": "due_live_deadline",
    "rust-server/src/app/session/timing.rs": "deadlines due_deadline",
    "rust-server/src/agent_experiment/execution.rs": "execute_plans",
    "rust-server/src/agent_experiment/decisions.rs": "record_trajectory",
    "rust-server/src/agent_experiment.rs": "run_yaml",
    "rust-server/src/agent_experiment/tests/resume.rs": "yaml_run_executes_once_and_resume_skips_finalized_result",
    "rust-server/src/auth.rs": "UpgradeTicketStore consume participant_credentials_resolve_to_the_issued_principal",
    "rust-server/src/audio.rs": "replacement_connection_owns_role_until_its_own_disconnect",
    "parlando-agent-sdk/src/parlando_agent_sdk/server.py": "_AgentService Respond",
    "rust-server/src/game.rs": "Game GameFactory",
    "experiments/cue-choice-rl/server/src/game.rs": "apply_action observation complete_session_preserves_private_cue_and_role_rules",
    "rust-server/src/protocol.rs": "ParticipantState SessionEnd",
    "rust-server/src/storage.rs": "SqliteExperimentStore",
    "rust-server/src/storage/events.rs": "commit_session_transition end_session append_session_event",
    "rust-server/src/storage/export.rs": "read_export_snapshot",
    "rust-server/src/app/corpus.rs": "corpus_experiment_export",
    "rust-server/src/remote_agent.rs": "respond",
    "js-client/src/participantConnection.ts": "ParticipantConnection act acceptsInput reconcile scheduleRetry",
    "js-client/src/protocol.ts": "sendAction",
    "rust-server/src/app/tests/session_transitions.rs": "reconnect_within_grace_after_idle_deadline_returns_ended_room concurrent_agent_room_joins_reserve_one_session already_ended_storage_never_publishes_an_accepted_action simultaneous_actions_publish_role_private_snapshots_in_replay_order slow_agent_never_blocks_another_room_or_loses_an_accepted_action",
    "rust-server/src/app/tests/speech_timing.rs": "generated_and_recognized_speech_keep_audio_clock_timings_end_to_end",
    "rust-server/src/app/tests/mounts.rs": "mounted_room_reconnects_across_intake_and_config_changes",
    "rust-server/src/app/tests/reconnection.rs": "brief_disconnect_pauses_partner_and_reconnect_preserves_session disconnect_past_grace_gives_partner_a_terminal_outcome",
    "rust-server/src/app/tests/gameplay.rs": "recorded_session_reconstructs_without_per_action_states",
    "rust-server/src/app/tests/corpus.rs": "corpus_export_is_nested_informative_and_structurally_minimized",
    "rust-server/src/app/tests/agent_play.rs": "agent_runtime_observes_messages_with_speaker_and_modality",
    "rust-server/src/storage/tests/transitions.rs": "concurrent_terminal_transitions_have_exactly_one_winner",
    "client-server-tests/tests/browser_e2e.rs": "real_browsers_exercise_the_participant_state_machine",
}


def bundle():
    """Resolve each named definition once and fail if Graft cannot locate it uniquely."""
    records = {}
    for path, names in FILES.items():
        graph = subprocess.check_output(["graft", "skeleton", path], cwd=ROOT, text=True)
        lines = (ROOT / path).read_text().splitlines()
        for name in names.split():
            matches = re.findall(rf"^- L(\d+)-L(\d+)\s+\w+ {name}  ", graph, re.M)
            if len(matches) != 1 or name in records:
                raise ValueError(f"Ambiguous or missing symbol: {path} {name}")
            first, last = map(int, matches[0])
            records[name] = dict(path=path, first=first, lines=lines[first - 1:last])
    return records


if __name__ == "__main__":
    payload = json.dumps(bundle(), ensure_ascii=False)
    (ATLAS / "sources.js").write_text(f"const sources = {payload};\n")
