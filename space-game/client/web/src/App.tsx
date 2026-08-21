import { useCallback, useEffect, useMemo, useState } from "react";
import { type PlayerMessage, type VoicePreflight, type VoiceStatus } from "@coli-saar/parlando-client";
import {
  MicrophoneLevelMeter,
  ParticipantApp,
  MicrophoneMuteButton,
  type GameSession
} from "@coli-saar/parlando-client/react";
import {
  cells,
  devices,
  doors,
  mapHeight,
  mapWidth,
  roomAtPosition,
  roomById,
  roomRegions
} from "./game/level";
import { availableActions, deriveSystems, describeAction } from "./game/stateEngine";
import type { DeviceDefinition, Direction, GameAction, PlayerId, Position, StationState } from "./game/types";
import type { StationObservation } from "./game/types";
const movementKeys: Record<string, Direction> = {
  arrowup: "up",
  arrowdown: "down",
  arrowleft: "left",
  arrowright: "right"
};

/** Removes browser-added USB identifiers from the displayed microphone name. */
function participantMicrophoneLabel(label: string): string {
  return label.replace(/\s*[([][0-9a-f]{4}:[0-9a-f]{4}[)\]]\s*$/i, "").trim() || "Microphone";
}

type SpaceGameSession = GameSession<StationObservation, GameAction>;

export function App() {
  return (
    <main className="app-shell">
      <ParticipantApp<StationObservation, GameAction>
        renderGame={(session) => <ActiveSpaceGame session={session} />}
      />
    </main>
  );
}

function ActiveSpaceGame({ session }: { session: SpaceGameSession }) {
  const [preview, setPreview] = useState<ActionPreview | null>(null);
  const [chatDraft, setChatDraft] = useState("");
  const state = session.observation;
  const systems = useMemo(() => deriveSystems(state), [state]);
  const serverAvailableActions = session?.availableActions ?? [];
  const assignedRole = session?.role === "A" || session?.role === "B" ? session.role : null;
  const voiceEnabled = session.voiceEnabled;
  const status = session.voiceStatus;
  const voicePreflight = session.voicePreflight;
  const onlineReady = true;
  const dispatch = useCallback(
    (action: GameAction) => {
      session.sendAction(action);
    },
    [session]
  );

  const submitChat = useCallback(() => {
    const text = chatDraft.trim();
    if (!text || !session) return;
    session.sendMessage(text);
    setChatDraft("");
  }, [chatDraft, session]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!session || !onlineReady) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, button")) return;
      const key = event.key.toLowerCase();
      const direction = movementKeys[key];
      if (direction && assignedRole) {
        event.preventDefault();
        dispatch({ type: "moveStep", player: assignedRole, direction });
        return;
      }

      if (key === "enter" && assignedRole) {
        const firstAction = session.availableActions?.[0] ?? availableActions(state, assignedRole)[0];
        if (firstAction) {
          event.preventDefault();
          dispatch(firstAction);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [assignedRole, dispatch, onlineReady, session, state]);

  return (
    <>
      <section className="status-band" aria-label="Station systems">
        <div>
          <p className="eyebrow">Evacuation Beacon</p>
          <h1>{state.beaconLaunched ? "Carrier Lock Achieved" : "Station Repair"}</h1>
        </div>
        <SystemPill label="Power" online={systems.powerStable} />
        <SystemPill label="Oxygen" online={systems.oxygenStable} />
        <SystemPill label="Door" online={systems.doorAccess} />
        <SystemPill label="Signal" online={systems.signalRouted} />
        <SystemPill label="Cooling" online={systems.coolingRestored} />
      </section>

      <section className="session-band" aria-label="Online session controls">
        <div>
          <p className="eyebrow">Online room</p>
          <strong>{session.sessionId}</strong>
          <span>
            {session.connected ? "Connected" : "Disconnected"} · You are Player {session.role}
            {session.completed && " · Complete"}
          </span>
        </div>
        <button onClick={session.leave}>Leave game</button>
      </section>

      <section className={`game-layout ${assignedRole ? `game-layout-${assignedRole.toLowerCase()}` : ""}`}>
        {assignedRole === "A" && (
          <PlayerPanel
            canControl={onlineReady}
            player="A"
            state={state}
            actions={serverAvailableActions}
            dispatch={dispatch}
            setPreview={setPreview}
          />
        )}
        <section className="playfield-zone" aria-label="Top-down station playfield">
          <StationPlayfield preview={preview} state={state} systems={systems} />
          <SharedConsole preview={preview} state={state} systems={systems} />
          <CommunicationPanel
            chatDraft={chatDraft}
            conversation={session.conversation}
            enabled={voiceEnabled}
            onChatDraftChange={setChatDraft}
            onSubmitChat={submitChat}
            onMutedChange={(muted) => void session.setMicrophoneMuted(muted).catch(() => undefined)}
            voicePreflight={voicePreflight}
            status={status}
          />
        </section>
        {assignedRole === "B" && (
          <PlayerPanel
            canControl={onlineReady}
            player="B"
            state={state}
            actions={serverAvailableActions}
            dispatch={dispatch}
            setPreview={setPreview}
          />
        )}
      </section>
    </>
  );
}

function PlayerPanel({
  canControl,
  player,
  state,
  actions,
  dispatch,
  setPreview
}: {
  canControl: boolean;
  player: PlayerId;
  state: StationState;
  actions: GameAction[];
  dispatch: (action: GameAction) => void;
  setPreview: (preview: ActionPreview | null) => void;
}) {
  const room = roomById[state.players[player].room];
  const controls = "Arrow keys + Enter";

  return (
    <aside className={`player-panel player-${player.toLowerCase()}`}>
      <div className="panel-heading">
        <p className="eyebrow">Player {player}</p>
        <h2>{room.name}</h2>
        <span className="control-hint">{controls}</span>
      </div>
      <div className="private-note">
        <span>Private schematic</span>
        <p>{playerBriefing(player)}</p>
      </div>
      <div className="movement-pad" aria-label={`Player ${player} movement controls`}>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "up" })}>
          ↑
        </button>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "left" })}>
          ←
        </button>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "down" })}>
          ↓
        </button>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "right" })}>
          →
        </button>
      </div>
      <div className="action-grid">
        {!canControl ? (
          <p className="no-actions">This browser is assigned to the other player.</p>
        ) : actions.length === 0 ? (
          <p className="no-actions">Stand on a device tile to use it. Floor plates work automatically.</p>
        ) : (
          actions.map((action, index) => (
            <button
              className={action.type === "launchBeacon" ? "critical-action" : ""}
              key={`${describeAction(action, state)}-${index}`}
              onClick={() => {
                setPreview(null);
                dispatch(action);
              }}
              onBlur={() => setPreview(null)}
              onFocus={() => setPreview(actionPreview(action, state))}
              onMouseEnter={() => setPreview(actionPreview(action, state))}
              onMouseLeave={() => setPreview(null)}
            >
              <span className="action-icon" aria-hidden="true">
                {actionIcon(action)}
              </span>
              <span>{describeAction(action, state)}</span>
            </button>
          ))
        )}
      </div>
      <section className="knowledge-list" aria-label={`Player ${player} discovered knowledge`}>
        <h3>Recent clues</h3>
        <ul>
          {state.knowledge[player].slice(-3).map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      </section>
    </aside>
  );
}

function StationPlayfield({
  preview,
  state,
  systems
}: {
  preview: ActionPreview | null;
  state: StationState;
  systems: ReturnType<typeof deriveSystems>;
}) {
  const visibleDevices = devicesForState(state);

  return (
    <div
      className="station-playfield"
      style={{
        gridTemplateColumns: `repeat(${mapWidth}, minmax(0, 1fr))`,
        gridTemplateRows: `repeat(${mapHeight}, minmax(0, 1fr))`
      }}
    >
      {roomRegions.map((room) => (
        <div
          className={`room-region region-${room.id} ${roomEffectClass(room.id, systems, state)} ${
            preview?.rooms.includes(room.id) ? "preview-target" : ""
          }`}
          key={room.id}
          style={{
            gridColumn: `${room.left + 1} / ${room.right + 2}`,
            gridRow: `${room.top + 1} / ${room.bottom + 2}`
          }}
        >
          <span>{room.name}</span>
        </div>
      ))}
      {doors.map((door) => (
        <div
          aria-label={door.label}
          className={[
            "door",
            door.a.x === door.b.x ? "door-vertical" : "door-horizontal",
            door.kind === "pressure" && !systems.doorAccess ? "door-locked" : "door-open"
          ].join(" ")}
          key={door.id}
          style={{
            gridColumn: `${Math.min(door.a.x, door.b.x) + 1} / ${Math.max(door.a.x, door.b.x) + 2}`,
            gridRow: `${Math.min(door.a.y, door.b.y) + 1} / ${Math.max(door.a.y, door.b.y) + 2}`
          }}
          title={door.label}
        />
      ))}
      {cells().map((cell) => {
        const room = roomAtPosition(cell);
        const device = visibleDevices.find((candidate) => samePosition(candidate.position, cell));
        const players = (["A", "B"] as PlayerId[]).filter((player) =>
          samePosition(state.players[player].position, cell)
        );
        const deviceIsNearby = device && isNearPlayer(device.position, state);
        const deviceIsUnderPlayer = device && isPlayerOn(device.position, state);
        const sealedAirlock =
          room === "airlock" && !systems.doorAccess && (cell.x === 5 || cell.x === 7 || cell.y === 8);

        return (
          <div
            className={[
              "map-cell",
              room ? "floor" : "void",
              room ? `cell-${room}` : "",
              device ? "has-device" : "",
              state.visualEffects.includes(`room:${room}`) ? "effect-flash" : "",
              deviceIsNearby ? "device-nearby" : "",
              deviceIsUnderPlayer ? "device-ready" : "",
              sealedAirlock ? "sealed" : ""
            ].join(" ")}
            key={`${cell.x}-${cell.y}`}
            style={{ gridColumn: cell.x + 1, gridRow: cell.y + 1 }}
          >
            {room && <span className="floor-grid" aria-hidden="true" />}
            {device && (
              <DeviceSprite
                device={device}
                isPreviewed={Boolean(preview?.devices.includes(device.id))}
                state={state}
                systems={systems}
              />
            )}
            {players.map((player) => (
              <span key={player} className={`player-token token-${player.toLowerCase()}`}>
                {player}
              </span>
            ))}
          </div>
        );
      })}
    </div>
  );
}

function DeviceSprite({
  device,
  isPreviewed,
  state,
  systems
}: {
  device: DeviceDefinition;
  isPreviewed: boolean;
  state: StationState;
  systems: ReturnType<typeof deriveSystems>;
}) {
  return (
    <span
      className={`device device-${device.kind} device-${device.id} ${deviceStateClass(device, state, systems)} ${
        state.visualEffects.includes(`device:${device.id}`) ? "effect-flash" : ""
      } ${isPreviewed ? "preview-target" : ""}`}
      title={device.label}
      aria-label={device.label}
    >
      {deviceGlyph(device, state)}
    </span>
  );
}

function SharedConsole({
  preview,
  state,
  systems
}: {
  preview: ActionPreview | null;
  state: StationState;
  systems: ReturnType<typeof deriveSystems>;
}) {
  const recentEvents =
    state.log.length > 0
      ? state.log
      : ["Emergency lamps are on. The beacon checklist is dark: power, oxygen, access, signal."];
  return (
    <section className="console-grid simplified-console">
      <div className="objective-card">
        <p className="eyebrow">{preview ? "Action preview" : "Goal"}</p>
        <h3>{preview?.title ?? (systems.readyToLaunch ? "Launch the beacon" : "Restore all systems")}</h3>
        <p>
          {preview?.summary ??
            "Hover an action to light up what it will affect. Stand on the floor plate to hold the hatch latch."}
        </p>
      </div>
      <div className="event-log">
        <h3>Recent events</h3>
        <ol>
          {recentEvents.slice(0, 4).map((line, index) => (
            <li key={`${line}-${index}`}>{line}</li>
          ))}
        </ol>
      </div>
    </section>
  );
}

/** Renders Space Game text or voice communication with explicit local mute feedback. */
export function CommunicationPanel({
  chatDraft,
  conversation,
  enabled,
  onChatDraftChange,
  onSubmitChat,
  onMutedChange,
  voicePreflight,
  status
}: {
  chatDraft: string;
  conversation: PlayerMessage[];
  enabled: boolean;
  onChatDraftChange: (value: string) => void;
  onSubmitChat: () => void;
  onMutedChange: (muted: boolean) => void;
  voicePreflight: VoicePreflight;
  status: VoiceStatus;
}) {
  const textMessages = conversation.filter((message) => message.input === "text").slice(-6);
  if (!enabled) {
    return (
      <section className="communication-panel" aria-label="Text chat">
        <div className="communication-header">
          <div>
            <p className="eyebrow">Comms</p>
            <h3>Text chat</h3>
          </div>
        </div>
        <ol className="conversation-list">
          {textMessages.length === 0 ? (
            <li className="conversation-empty">No chat yet.</li>
          ) : (
            textMessages.map((message) => (
              <li className={`conversation-message input-${message.input}`} key={message.id}>
                <span>Player {message.sender}</span>
                <p>{message.text}</p>
              </li>
            ))
          )}
        </ol>
        <form
          className="chat-form"
          onSubmit={(event) => {
            event.preventDefault();
            onSubmitChat();
          }}
        >
          <input
            aria-label="Text chat message"
            onChange={(event) => onChatDraftChange(event.target.value)}
            placeholder="Type to your partner"
            value={chatDraft}
          />
          <button disabled={!chatDraft.trim()} type="submit">
            Send
          </button>
        </form>
      </section>
    );
  }

  const microphoneMuted = status.connected && !status.microphoneEnabled;
  const microphoneLive = status.connected && status.microphoneEnabled;
  const microphoneStateLabel = microphoneMuted
    ? "Muted"
    : microphoneLive
      ? "Live"
      : status.connecting
        ? "Connecting"
        : "Offline";
  return (
    <section className={`communication-panel ${microphoneMuted ? "microphone-muted" : ""}`} aria-label="Voice chat">
      <div className="communication-header">
        <div>
          <p className="eyebrow">Microphone</p>
        </div>
        <MicrophoneMuteButton
          enabled={enabled}
          onMutedChange={onMutedChange}
          status={status}
        />
      </div>
      <div className="voice-feedback" aria-label="Voice diagnostics">
        <div className="meter-stack">
          <div className="microphone-name-row">
            <span className="mic-device-label">{participantMicrophoneLabel(voicePreflight.deviceLabel)}</span>
            <span className={`microphone-name-state ${microphoneStateLabel.toLowerCase()}`}>
              {microphoneStateLabel}
            </span>
          </div>
          <MicrophoneLevelMeter
            active={voicePreflight.micProbeActive}
            label="Level"
            level={voicePreflight.micLevel}
            muted={!microphoneLive}
          />
          <div
            aria-live="polite"
            className={`microphone-status-chip ${microphoneStateLabel.toLowerCase()}`}
          >
            <span aria-hidden="true" />
            {microphoneStateLabel}
          </div>
        </div>
      </div>
      {status.error && <p className="voice-error" role="alert">{status.error}</p>}
    </section>
  );
}

function SystemPill({ label, online }: { label: string; online: boolean }) {
  return (
    <div className={`system-pill ${online ? "online" : "offline"}`}>
      <span aria-hidden="true" />
      {label}
    </div>
  );
}

interface ActionPreview {
  title: string;
  summary: string;
  devices: string[];
  rooms: string[];
}

function samePosition(a: Position, b: Position): boolean {
  return a.x === b.x && a.y === b.y;
}

function playerBriefing(player: PlayerId): string {
  if (player === "A") {
    return "Electrical sheet: blue feeds the bus, yellow feeds the pump, AUX can backfeed the pump line.";
  }
  return "Flow sheet: valve A refills pressure, valve C trades charger water against coolant flow.";
}

function actionPreview(action: GameAction, state: StationState): ActionPreview {
  switch (action.type) {
    case "toggleFuse": {
      const devices = [`fuse-${action.color}`];
      const rooms = ["power"];
      const inserts = !state.fuses[action.color];
      if (action.color === "yellow" && inserts && state.breakers.aux) rooms.push("oxygen");
      return {
        title: `${inserts ? "Insert" : "Remove"} ${action.color} fuse`,
        summary:
          action.color === "blue"
            ? "Power Bay bus lights are affected."
            : action.color === "yellow"
              ? "Pump power changes; Oxygen may react if AUX is on."
              : "Reserve fuse changes only this rack.",
        devices,
        rooms
      };
    }
    case "toggleBreaker": {
      const turnsOn = !state.breakers[action.breaker];
      return {
        title: `Turn ${action.breaker.toUpperCase()} ${turnsOn ? "on" : "off"}`,
        summary:
          action.breaker === "main"
            ? "Main power lighting and battery charger feed are affected."
            : "AUX can backfeed the pump line if yellow fuse is inserted.",
        devices: [`breaker-${action.breaker}`],
        rooms: action.breaker === "main" ? ["power", "charger"] : ["power", "oxygen"]
      };
    }
    case "setValve":
      return {
        title: `${action.open ? "Open" : "Close"} valve ${action.valve}`,
        summary:
          action.valve === "A"
            ? "Cabin pressure in Oxygen changes."
            : action.valve === "C"
              ? "Water/coolant route changes between Charger and Valve Room."
              : "Coolant comes online, but pressure can drain.",
        devices: [`valve-${action.valve.toLowerCase()}`],
        rooms: action.valve === "A" ? ["valve", "oxygen"] : ["valve", "charger"]
      };
    case "holdOverride":
      return {
        title: `${action.held ? "Hold" : "Release"} bypass`,
        summary: "The airlock door motor stays powered while the battery is away.",
        devices: ["bypass"],
        rooms: ["junction", "airlock"]
      };
    case "chargeBattery":
      return {
        title: "Start battery charger",
        summary: "The charger and battery will visibly light if power, pump, and water are ready.",
        devices: ["charger", "battery"],
        rooms: ["charger"]
      };
    case "moveBattery":
      return {
        title: state.battery.location === "charger" ? "Move battery to Signal Array" : "Return battery to Charger",
        summary: "The BAT tile moves rooms; the airlock may lose motor power unless bypass is held.",
        devices: ["battery"],
        rooms: state.battery.location === "charger" ? ["charger", "signal", "airlock"] : ["signal", "charger"]
      };
    case "cycleRelay":
      return {
        title: `Rotate relay to ${nextRelayLabel(state)}`,
        summary: "The Signal Array routing label changes on the relay tile.",
        devices: ["relay"],
        rooms: ["signal"]
      };
    case "launchBeacon":
      return {
        title: "Launch beacon",
        summary: "Airlock and Signal Array react. If systems are not ready, the battery pulse is spent.",
        devices: ["beacon", "battery"],
        rooms: ["airlock", "signal"]
      };
    default:
      return {
        title: describeAction(action, state),
        summary: "This affects the highlighted object or room.",
        devices: [],
        rooms: []
      };
  }
}

function nextRelayLabel(state: StationState): string {
  if (state.relay === "bypass") return "LOOP";
  if (state.relay === "loop") return "ARRAY";
  return "BYPASS";
}

function isNearPlayer(position: Position, state: StationState): boolean {
  return (["A", "B"] as PlayerId[]).some((player) => {
    const playerPosition = state.players[player].position;
    return Math.abs(playerPosition.x - position.x) + Math.abs(playerPosition.y - position.y) <= 1;
  });
}

function isPlayerOn(position: Position, state: StationState): boolean {
  return (["A", "B"] as PlayerId[]).some((player) => samePosition(state.players[player].position, position));
}

function devicesForState(state: StationState): DeviceDefinition[] {
  return devices.map((device) => {
    if (device.id === "battery" && state.battery.location === "signal") {
      return { ...device, room: "signal", position: { x: 8, y: 8 } };
    }
    return device;
  });
}

function roomEffectClass(
  room: string,
  systems: ReturnType<typeof deriveSystems>,
  state: StationState
): string {
  if (room === "power" && systems.powerStable) return "room-powered";
  if (room === "oxygen" && systems.oxygenStable) return "room-oxygen";
  if (room === "oxygen" && state.oxygenFanTripped) return "room-warning";
  if (room === "charger" && systems.chargerFed) return "room-powered";
  if (room === "valve" && systems.coolingRestored) return "room-cooling";
  if (room === "signal" && systems.signalRouted) return "room-signal";
  if (room === "airlock" && systems.doorAccess) return "room-door";
  if (room === "airlock" && state.battery.spent) return "room-warning";
  return "";
}

function deviceGlyph(device: DeviceDefinition, state: StationState): string {
  if (device.id === "fuse-blue") return "BLU";
  if (device.id === "fuse-yellow") return "YEL";
  if (device.id === "fuse-red") return "RED";
  if (device.id === "breaker-main") return "MAIN";
  if (device.id === "breaker-aux") return "AUX";
  if (device.id === "valve-a") return state.valves.A ? "OPEN" : "SHUT";
  if (device.id === "valve-c") return state.valves.C ? "WATR" : "COOL";
  if (device.id === "valve-floodgate") return state.valves.floodgate ? "OPEN" : "SHUT";
  if (device.kind === "bypass") return state.overrideHeld ? "HELD" : "PULL";
  if (device.kind === "plate") return "▣";
  if (device.kind === "charger") return state.battery.charged ? "DONE" : "CHG";
  if (device.kind === "battery") return state.battery.charged ? "BAT+" : "BAT";
  if (device.kind === "relay") {
    if (state.relay === "bypass") return "BYP";
    if (state.relay === "array") return "ARR";
    return "LOOP";
  }
  if (device.kind === "diagnostic") return "?";
  if (device.kind === "beacon") return state.beaconLaunched ? "SENT" : "LAUN";
  return "•";
}

function deviceStateClass(
  device: DeviceDefinition,
  state: StationState,
  systems: ReturnType<typeof deriveSystems>
): string {
  if (device.id === "fuse-blue") return state.fuses.blue ? "active" : "";
  if (device.id === "fuse-yellow") return state.fuses.yellow ? "active yellow" : "";
  if (device.id === "fuse-red") return state.fuses.red ? "active red" : "";
  if (device.id === "breaker-main") return state.breakers.main ? "active" : "";
  if (device.id === "breaker-aux") return state.breakers.aux ? "active yellow" : "";
  if (device.id === "valve-a") return state.valves.A ? "active teal" : "";
  if (device.id === "valve-c") return state.valves.C ? "active teal" : "";
  if (device.id === "valve-floodgate") return state.valves.floodgate ? "active teal" : "";
  if (device.id === "bypass") return state.overrideHeld ? "active yellow" : "";
  if (device.id === "plate") return state.players.B.plateHeld ? "active yellow" : "";
  if (device.id === "charger") return systems.chargerFed ? "active" : "";
  if (device.id === "battery") return state.battery.charged ? "active yellow" : "";
  if (device.id === "relay") return state.relay === "array" ? "active green" : state.relay === "loop" ? "active yellow" : "";
  if (device.id === "beacon") return state.beaconLaunched ? "active green" : "";
  return "";
}

function actionIcon(action: GameAction): string {
  const icons: Record<GameAction["type"], string> = {
    moveStep: "↕",
    toggleFuse: "▮",
    toggleBreaker: "⏻",
    setValve: "◌",
    holdOverride: "⫷",
    togglePlate: "▣",
    chargeBattery: "⚡",
    moveBattery: "⇥",
    setRelay: "⌁",
    cycleRelay: "⌁",
    runDiagnostic: "?",
    launchBeacon: "▲"
  };
  return icons[action.type];
}
