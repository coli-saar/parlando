// The entry symbol identifies a useful reading point. The additional symbols locate collaborators.
const parts = {
  host: {
    job: "Select the experiment that owns a request",
    input: "Experiment ID and participant registration",
    output: "A request handler using the configuration that applies to this participant",
    why: "If an administrator reopens intake with different settings, existing sessions continue under their original configuration. The host routes a returning participant to the runtime that holds their session.",
    symbols: ["GameHost", "experiment_router", "participant_owner", "AppState"],
  },
  identity: {
    job: "Authenticate a participant’s game socket",
    input: "A browser request to open a WebSocket and the single-use ticket that authorizes it",
    output: "A checked participant identity, session and role",
    why: "A participant can replace a disconnected socket. The server checks that the ticket, current credential, and assigned role belong to the returning participant.",
    symbols: ["UpgradeTicketStore", "consume", "game_socket", "websocket_loop"],
  },
  rooms: {
    job: "Manage session membership and deadlines",
    input: "Admissions, seat readiness, disconnects and elapsed deadlines",
    output:
      "The session’s status and the view of that status sent to each participant",
    why: "An experiment can host many matches. Each needs its own game instance, player assignments, deadlines and lock so that a change in one match has a well-defined order.",
    symbols: [
      "LiveSession",
      "admit_session",
      "maybe_start_game",
      "participant_state_response",
    ],
  },
  browser: {
    job: "Maintain the participant’s view and send actions",
    input: "Participant snapshots, socket messages and local UI actions",
    output: "A synchronized client view and outbound action messages",
    why: "A browser can reload or lose its socket while the session keeps running. It must reconcile with the server before re-enabling input.",
    symbols: ["ParticipantConnection", "act", "sendAction", "reconcile"],
  },
  transport: {
    job: "Carry a player’s input into the session",
    input: "Frames from the socket currently assigned to the role",
    output:
      "Decoded commands for runtime handlers and packets for the connected player",
    why: "The same participant can open a replacement connection. Cleanup from the earlier socket must check ownership before changing presence.",
    symbols: [
      "websocket_loop",
      "handle_client_message",
      "unregister",
    ],
  },
  transitions: {
    job: "Decide whether a proposed move takes effect",
    input: "A session ID, participant seat and typed game action",
    output: "A committed move with per-role notifications, or a rejection",
    why: "Two players may act while a timeout is trying to end the session. One per-session lock orders these operations, and the database must accept the move before the runtime publishes it.",
    symbols: ["submit_action", "prepare_action", "reserve_agent_delivery"],
  },
  game: {
    job: "Define what a move means",
    input: "Game state, typed action and acting role",
    output:
      "New mechanics state or a rule rejection, plus observations derived for each role",
    why: "The server can run different two-player games because each game supplies its own mechanics and controls which facts each role may see.",
    symbols: ["Game", "step", "observation"],
  },
  storage: {
    job: "Record what the session accepted",
    input: "Session-scoped events and an optional terminal result",
    output: "Ordered event rows and an atomic commit outcome",
    why: "A live session disappears on restart. Its accepted moves and the result each participant received must remain available for reconciliation and analysis.",
    symbols: [
      "SqliteExperimentStore",
      "commit_session_transition",
      "end_session",
      "read_export_snapshot",
    ],
  },
  agents: {
    job: "Assign an agent to a role",
    input: "Ordered role observations and messages from the runtime",
    output:
      "A proposed action or message submitted through the ordinary session handlers",
    why: "The agent runs in a separate task. The runtime queues observations for that task, so a slow policy can finish its previous decision before reading the next observation.",
    symbols: [
      "start_agent_task",
      "handle_agent_response",
      "reserve_agent_delivery",
    ],
  },
  sdk: {
    job: "Call a policy in a separate process",
    input:
      "A gRPC decision request carrying the agent instance and available actions",
    output: "The policy response and buffered session logs",
    why: "A policy is the code an agent uses to decide how to act. It can run in a Python process while Rust manages the session. Both adapters must agree on the meaning of a missing decision and the list of available actions.",
    symbols: ["_AgentService", "Respond", "respond"],
  },
  messages: {
    job: "Record conversation messages",
    input: "Speaker identity, text, origin and optional source metadata",
    output:
      "A stored message, plus the text and speaker information delivered to players",
    why: "The stored record identifies the speaker and whether the message came from typing, speech recognition, or an agent. Later readers can recover the conversation in session order.",
    symbols: ["commit_conversation_message", "commit_final_transcript"],
  },
  audio: {
    job: "Relay one role’s audio to its partner",
    input: "Binary audio frames on the audio socket",
    output: "Partner audio, plus recognition input when enabled and active",
    why: "The server sends audio to the other player and can also feed a speech recognizer. The recognizer has a limited input queue. If it falls behind, its transcription may omit audio that the partner heard.",
    symbols: [
      "audio_websocket_loop",
      "replacement_connection_owns_role_until_its_own_disconnect",
    ],
  },
  speech: {
    job: "Store a recognized utterance as a message",
    input: "A finalized utterance with provider IDs and timing",
    output:
      "A conversation record with a transcript source ID and timing metadata",
    why: "Speech recognition can repeat a result. The handler remembers the recognition result’s ID to keep a repeat from becoming a second message. If storage fails, it releases the ID so the result can be retried.",
    symbols: ["commit_final_transcript", "commit_conversation_message"],
  },
  analysis: {
    job: "Reconstruct a session or export its records",
    input:
      "Initial mechanics state, accepted actions or an experiment snapshot",
    output: "Per-role replay frames or an exported research corpus",
    why: "Replay uses the initial game state and accepted actions to reconstruct each role’s observations. Export reads the stored experiment records and selects fields for the corpus.",
    symbols: [
      "admin_session_replay",
      "admin_export",
      "corpus_experiment_export",
    ],
  },
  batch: {
    job: "Run agent games without a browser session",
    input: "A YAML experiment and its session plans",
    output: "One result file per planned session, optionally including the sequence of observations, moves, and rewards used for training",
    why: "Repeated agent experiments need bounded concurrency and resumable work. The batch runner shares the Game interface while owning its own scheduling and persistence.",
    symbols: ["run_yaml", "execute_plans", "record_trajectory"],
  },
};
const readingGroups = [
  [
    "Admission",
    "The host chooses the experiment. Admission assigns a participant to one session and one role.",
    ["host", "identity", "rooms"],
  ],
  [
    "Game execution",
    "One browser proposes the action. The server evaluates it and records the outcome before sending each player a view.",
    ["browser", "transport", "transitions", "game", "storage"],
  ],
  [
    "Agents and communication",
    "A software seat and a voice channel add work around the same session. Neither requires new game rules.",
    ["agents", "sdk", "messages", "audio", "speech"],
  ],
  [
    "Replay, export, and batch experiments",
    "Replay consumes a recorded history. The batch runner creates new histories under its own scheduler.",
    ["analysis", "batch"],
  ],
];
// Cross-cutting properties exercised by fixtures outside their primary subsystem.
const sharedFixtures = {
  reconnect_within_grace_after_idle_deadline_returns_ended_room: [
    "transport",
    "browser",
    "transitions",
  ],
  simultaneous_actions_publish_role_private_snapshots_in_replay_order: [
    "transitions",
    "transport",
    "analysis",
  ],
  concurrent_terminal_transitions_have_exactly_one_winner: [
    "transitions",
    "rooms",
  ],
  slow_agent_never_blocks_another_room_or_loses_an_accepted_action: [
    "transitions",
    "rooms",
  ],
  already_ended_storage_never_publishes_an_accepted_action: [
    "storage",
    "transport",
  ],
  brief_disconnect_pauses_partner_and_reconnect_preserves_session: [
    "rooms",
    "browser",
  ],
  disconnect_past_grace_gives_partner_a_terminal_outcome: [
    "transport",
    "browser",
  ],
  recorded_session_reconstructs_without_per_action_states: ["game", "storage"],
  agent_runtime_observes_messages_with_speaker_and_modality: ["agents"],
  generated_and_recognized_speech_keep_audio_clock_timings_end_to_end: [
    "audio",
    "messages",
  ],
};
const assertions = {
  reconnect_within_grace_after_idle_deadline_returns_ended_room:
    'assert_eq!(ended["result"]["reason"], "idle_timeout")',
  yaml_run_executes_once_and_resume_skips_finalized_result:
    "assert_eq!(second.skipped, 1)",
  replacement_connection_owns_role_until_its_own_disconnect:
    'assert!(!sessions.is_current("session", "A", &old_generation)',
  participant_credentials_resolve_to_the_issued_principal: "assert_eq!(",
  generated_and_recognized_speech_keep_audio_clock_timings_end_to_end:
    'assert_eq!(after["tokens"][0]["confidence"], 0.9)',
  agent_runtime_observes_messages_with_speaker_and_modality:
    'if captured.contains(&"message:A:typed hello"',
  concurrent_agent_room_joins_reserve_one_session:
    'assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1)',
  mounted_room_reconnects_across_intake_and_config_changes:
    "assert_eq!(same, room)",
  corpus_export_is_nested_informative_and_structurally_minimized:
    'assert!(!encoded.contains("recruitment-id"))',
  real_browsers_exercise_the_participant_state_machine:
    "|| !missing_transitions.is_empty()",
  already_ended_storage_never_publishes_an_accepted_action:
    'assert_no_ws_type(&mut second, "transition")',
  simultaneous_actions_publish_role_private_snapshots_in_replay_order:
    'assert_eq!(packet["observation"], frame["observation"])',
  concurrent_terminal_transitions_have_exactly_one_winner:
    "assert_eq!(terminal_events.len(), 1)",
  slow_agent_never_blocks_another_room_or_loses_an_accepted_action:
    'assert_eq!(observed.recv().await, Some("observed"))',
  brief_disconnect_pauses_partner_and_reconnect_preserves_session:
    'assert_eq!(reconnected["public_session_id"], public_session_id)',
  disconnect_past_grace_gives_partner_a_terminal_outcome:
    'assert_eq!(ended["result"]["reason"], "reconnect_timeout")',
  recorded_session_reconstructs_without_per_action_states:
    'assert_eq!(replay["roles"]["A"][2]["observation"]["done"], true)',
};
const actionFixtures = [
  "simultaneous_actions_publish_role_private_snapshots_in_replay_order",
  "already_ended_storage_never_publishes_an_accepted_action",
  "slow_agent_never_blocks_another_room_or_loses_an_accepted_action",
  "concurrent_terminal_transitions_have_exactly_one_winner",
];
// Contract keys describe caller and callee; each deep link opens the matching stage or investigation.
const contractRoutes = {
  "transport/identity": "guide/identity",
  "audio/speech": "flows/speech/0",
  "audio/rooms": "flows/admission/3",
  "agents/sdk": "guide/sdk",
  "batch/game": "flows/batch/2",
  "transport/transitions": "flows/action/1",
  "agents/transitions": "flows/action/1",
  "transitions/game": "flows/action/2",
  "transitions/storage": "flows/action/4",
  "transitions/agents": "flows/action/3",
  "transitions/transport": "flows/action/6",
  "browser/transport": "flows/action/0",
  "transport/browser": "flows/reconnect/2",
  "host/rooms": "flows/admission/0",
  "rooms/storage": "flows/admission/3",
  "transport/rooms": "incident",
  "rooms/transitions": "incident",
  "transport/messages": "flows/speech/1",
  "speech/messages": "flows/speech/0",
  "messages/storage": "flows/speech/2",
  "messages/agents": "flows/speech/3",
  "agents/messages": "guide/messages",
  "analysis/game": "flows/replay/1",
  "analysis/storage": "flows/replay/2",
};
const readingNotes = {
  GameHost:
    "Locate the game-wide router cache and shared installation services.",
  UpgradeTicketStore:
    "Locate the short-lived tickets held until a socket upgrade.",
  consume: "Check the ticket scope when consuming it.",
  ParticipantConnection:
    "Locate the browser’s current session, connection generation and retry timers.",
  SqliteExperimentStore: "Find the SQLite pool shared by the store operations.",
  _AgentService:
    "Locate the Python service that dispatches requests to agent instances.",
  experiment_router:
    "Build or reuse the runtime for an experiment configuration.",
  participant_owner:
    "Find the mount that already holds the participant’s session.",
  AppState:
    "Inspect the registries and locks shared by sessions in one experiment.",
  game_socket: "Follow a game-channel upgrade from ticket to checked role.",
  websocket_loop:
    "Follow connection replacement, message delivery and disconnect cleanup.",
  LiveSession:
    "Start with the fields that survive an individual socket connection.",
  admit_session: "Find the assignment under the experiment admission lock.",
  maybe_start_game: "See the durable start that precedes activation.",
  participant_state_response:
    "Translate one shared lifecycle into one role’s protocol state.",
  act: "Check input eligibility at the moment a player acts.",
  sendAction: "Encode the action and send it on the game socket.",
  reconcile:
    "Discard a stale HTTP response if the connection generation changed.",
  handle_client_message:
    "Validate wire input before decoding a game-specific action.",
  unregister:
    "Check which socket generation owns the slot being removed.",
  submit_action:
    "Follow the session lock from preparation through publication.",
  prepare_action:
    "Compute the candidate state and the packets for its recipients.",
  reserve_agent_delivery:
    "Reserve inbox space before accepting work that must reach the agent.",
  Game: "Read the associated types and methods required of every game.",
  step:
    "In the example game Cue Choice, one role deals a word and the other chooses its matching tree. This function checks whose turn it is and evaluates the choice.",
  observation:
    "After a deal, the chooser sees the word while the dealer sees a waiting status. This function builds both views from the same state.",
  commit_session_transition:
    "Read the terminal guard and the event batch transaction.",
  end_session: "Commit a terminal event and its recipient results.",
  read_export_snapshot: "Read related experiment rows within one transaction.",
  start_agent_task:
    "Follow a session’s observation receiver and policy callbacks.",
  handle_agent_response:
    "Route a returned move or message back through the runtime.",
  Respond: "Inspect the Python handler for one agent decision.",
  respond: "Inspect the Rust caller’s timeout, decode and log handling.",
  commit_conversation_message:
    "Trace the required event write before player and agent delivery.",
  commit_final_transcript:
    "Locate the deduplication key and its rollback on commit failure.",
  audio_websocket_loop:
    "Follow partner relay and the optional recognition input channel.",
  replacement_connection_owns_role_until_its_own_disconnect:
    "Inspect the fixture that closes a replaced audio connection.",
  admin_session_replay:
    "See how recorded inputs reconstruct the views shown during play.",
  admin_export:
    "Find the boundary between stored records and the requested export.",
  corpus_experiment_export: "Read the transformation into the research corpus.",
  run_yaml: "Read a run specification and resolve its inputs.",
  execute_plans:
    "Separate finalized results from pending plans before scheduling.",
  record_trajectory:
    "See which observations, actions and rewards training records retain.",
};
const incidentPresets = {
  idle: ["Idle first", 6, 15, 30],
  grace: ["Grace first", 20, 6, 30],
  lifetime: ["Lifetime first", 20, 15, 6],
  tie: ["All three coincide", 8, 8, 8],
};
