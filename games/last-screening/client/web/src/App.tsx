import { ParticipantApp } from "@coli-saar/parlando-client/react";
import { GameView } from "./GameView";
import type { Action, Completion, Observation } from "./game";

/** Provides the game surface and ending inside Parlando's participant lifecycle. */
export function App() {
  return (
    <ParticipantApp<Observation, Action, Completion>
      renderGame={(snapshot) => <GameView snapshot={snapshot} />}
      renderCompletion={() => (
        <div className="screening-ending">
          <p className="eyebrow">The Last Screening</p>
          <h2>You escaped together</h2>
          <p>You made it through the safety curtain. The picture palace goes dark behind you.</p>
        </div>
      )}
    />
  );
}
