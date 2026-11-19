import { ParticipantClient, type ExperimentInfo } from "@coli-saar/parlando-client";
import { ParticipantApp, type GameSnapshot } from "@coli-saar/parlando-client/react";

const client = new ParticipantClient({ baseUrl: "https://study.example/e/test" });
const experiment: Promise<ExperimentInfo> = client.getExperiment();

/** Compiles the public React game-snapshot contract as an external consumer. */
export function Consumer({ snapshot }: { snapshot: GameSnapshot<{ turn: number }, { type: "pass" }> }) {
  void experiment;
  return (
    <span>{snapshot.observation.turn}<button onClick={() => snapshot.act({ type: "pass" })}>Pass</button></span>
  );
}

export const App = () => <ParticipantApp<{ turn: number }, { type: "pass" }> renderGame={(snapshot) => <Consumer snapshot={snapshot} />} />;
