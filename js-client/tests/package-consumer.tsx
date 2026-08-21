import { ParticipantClient, type ExperimentInfo } from "@coli-saar/parlando-client";
import { MicrophoneMuteButton, type GameSession } from "@coli-saar/parlando-client/react";

const client = new ParticipantClient({ baseUrl: "https://study.example/e/test" });
const experiment: Promise<ExperimentInfo> = client.getExperiment();

/** Compiles the public React session and microphone component contracts as an external consumer. */
export function Consumer({ session }: { session: GameSession<{ turn: number }, { type: "pass" }> }) {
  void experiment;
  return (
    <>
      <span>{session.observation.turn}</span>
      <MicrophoneMuteButton enabled={session.voiceEnabled} status={session.voiceStatus} onMutedChange={session.setMicrophoneMuted} />
    </>
  );
}
