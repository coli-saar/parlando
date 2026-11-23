import { ParticipantApp, type GameSnapshot } from "@coli-saar/parlando-client/react";
import { CrownView } from "./game/CrownView";
import { RootView } from "./game/RootView";
import type { GreatTreeAction, GreatTreeCompletion, GreatTreeObservation } from "./game/types";

type Snapshot = Pick<GameSnapshot<GreatTreeObservation, GreatTreeAction>, "observation" | "interactionEnabled" | "act">;

export function App() {
  return (
    <main className="app-shell">
      <ParticipantApp<GreatTreeObservation, GreatTreeAction, GreatTreeCompletion>
        renderGame={(snapshot) => <ActiveGreatTree snapshot={snapshot} />}
        renderCompletion={(completion) => <GreatTreeSuccess completion={completion} />}
      />
    </main>
  );
}

/** Renders the role view and offers its controls only during an interactive session. */
export function ActiveGreatTree({ snapshot }: { snapshot: Snapshot }) {
  const observation = snapshot.observation;
  return (
    <div className="game-shell">
      <div className="game-panel">
        <p className="hint">Make three flowers bloom.</p>
        {observation.role === "crown" ? (
          <CrownView
            limbs={observation.limbs}
            onSetSun={snapshot.interactionEnabled ? (limb, lit) => snapshot.act({ type: "setSun", limb, lit }) : undefined}
          />
        ) : (
          <RootView
            roots={observation.roots}
            onSetFlow={snapshot.interactionEnabled ? (root, open) => snapshot.act({ type: "setFlow", root, open }) : undefined}
          />
        )}
      </div>
    </div>
  );
}

/** Describes Great Tree's cooperative win inside Parlando's standard terminal shell. */
export function GreatTreeSuccess({ completion }: { completion: GreatTreeCompletion }) {
  const count = completion.floweredLimbs.length;
  return (
    <div className="great-tree-success">
      <span aria-hidden="true" className="success-mark">✶</span>
      <p className="success-kicker">Success</p>
      <h1>The Great Tree is in bloom!</h1>
      <p>You worked together to bring the tree back to life.</p>
      <p className="success-count">{count} of five limbs flowered together.</p>
    </div>
  );
}
