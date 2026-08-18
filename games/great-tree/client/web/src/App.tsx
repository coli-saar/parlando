import {
  MicrophoneLevelMeter,
  MicrophoneMuteButton,
  ParticipantApp,
  type GameSession
} from "@coli-saar/parlando-client/react";
import { CrownView } from "./game/CrownView";
import { RootView } from "./game/RootView";
import type { GreatTreeAction, GreatTreeCompletion, GreatTreeObservation } from "./game/types";

type Session = GameSession<GreatTreeObservation, GreatTreeAction, GreatTreeCompletion>;

export function App() {
  return (
    <main className="app-shell">
      <ParticipantApp<GreatTreeObservation, GreatTreeAction, GreatTreeCompletion>
        renderGame={(session) => <ActiveGreatTree session={session} />}
      />
    </main>
  );
}

export function ActiveGreatTree({ session }: { session: Session }) {
  if (session.completed) {
    return <TerminalScreen completion={session.completion} onLeave={session.leave} />;
  }

  const observation = session.observation;
  return (
    <div className="game-shell">
      {observation.role === "crown" ? (
        <CrownView
          limbs={observation.limbs}
          onSetSun={(limb, lit) => session.sendAction({ type: "setSun", limb, lit })}
        />
      ) : (
        <RootView
          roots={observation.roots}
          onSetFlow={(root, open) => session.sendAction({ type: "setFlow", root, open })}
        />
      )}
      <SessionBar session={session} />
    </div>
  );
}

/**
 * Voice controls and the leave button, shown below the game during active play. Structurally
 * the same widget as space-game's CommunicationPanel: a header row (label + mute button) above
 * a meter row, reusing the same class names so the shared lifecycle CSS theming applies here too.
 */
function SessionBar({ session }: { session: Session }) {
  const microphoneMuted = session.voiceStatus.connected && !session.voiceStatus.microphoneEnabled;
  return (
    <section className={`communication-panel ${microphoneMuted ? "microphone-muted" : ""}`} aria-label="Voice chat">
      <div className="communication-header">
        <p className="eyebrow">Microphone</p>
        <MicrophoneMuteButton
          enabled={session.voiceEnabled}
          onMutedChange={(muted) => void session.setMicrophoneMuted(muted).catch(() => undefined)}
          status={session.voiceStatus}
        />
      </div>
      {session.voiceEnabled && (
        <div className="voice-feedback" aria-label="Voice diagnostics">
          <div className="meter-stack">
            <MicrophoneLevelMeter
              active={session.voicePreflight.micProbeActive}
              label="Level"
              level={session.voicePreflight.micLevel}
              muted={!session.voiceStatus.microphoneEnabled}
            />
          </div>
        </div>
      )}
      {session.voiceStatus.error && (
        <p className="voice-error" role="alert">
          {session.voiceStatus.error}
        </p>
      )}
      <button type="button" className="leave-button" onClick={session.leave}>
        Leave game
      </button>
    </section>
  );
}

function TerminalScreen({
  completion,
  onLeave
}: {
  completion: GreatTreeCompletion | null;
  onLeave: () => void;
}) {
  const count = completion?.floweredLimbs.length ?? 0;
  return (
    <div className="terminal-screen">
      <h1>The tree is in bloom.</h1>
      <p>{count} of five limbs flowered together.</p>
      <button type="button" className="leave-button" onClick={onLeave}>
        Leave game
      </button>
    </div>
  );
}
