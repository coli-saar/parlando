# Participant client integration

Use the React client and SDK version supplied by the scaffold. Use the
client's public API, and mirror the component's named action, observation,
and completion payloads in the browser. Match their serialized field and
variant names.

```tsx
import { ParticipantApp, type GameSnapshot } from "@coli-saar/parlando-client/react";

type Snapshot = GameSnapshot<GameObservation, GameAction>;

export function App() {
  return (
    <ParticipantApp<GameObservation, GameAction, GameCompletion>
      renderGame={(snapshot: Snapshot) => <GameView snapshot={snapshot} />}
    />
  );
}
```

Render the current view from `snapshot.observation`. Submit actions with
`snapshot.act` and gate interaction on `snapshot.interactionEnabled` and
any controls exposed in the view. The component independently validates
every action. `snapshot.transition` is nullable and describes the latest
accepted action when present. Use it for displays that need the actor or
action. The observation supplies the current task state.

Use `renderCompletion` for game-specific display of the shared result.
Let the SDK handle participant entry, connection and reconnect state,
conversation, audio, replay, and recruitment. Preserve these controls when
styling the app, including their accessible labels and focus states. Check
the actual markup from the project's SDK version.

Gate Prolific-specific copy on `snapshot.recruitmentSource === "prolific"`. This is the server-recorded admission source for this participant and survives reload and URL cleanup. Use neutral copy for `direct`, `local_preview`, and `null` (replay or unknown). An experiment’s recruitment settings, URL parameters, and the other player’s admission do not establish this seat’s source. Keep completion codes and return links in the SDK handoff widget. In a packaged-game pilot, leave the waiting room and reload the terminal screen through a direct admission. Verify neutral text throughout. On a Prolific admission, verify the applicable waiting instructions and the return action supplied by the server.

When upgrading a game built with 0.6.0, rebuild its browser assets with the matching client and replace the experiment’s old `voice.enabled` flag with `communication.mode`. A deployed game needs a new package revision for the updated browser assets.

### Place and style experiment communication

Use the same game UI for both values of `communication.mode`. With `typed`, the library displays the message history and composer. With `speech`, it displays the microphone meter, mute control, and status. Mode selection, provider access, and message delivery belong to Parlando. The standard placement below the game requires no code or stylesheet import.

For a custom layout, import `CommunicationWidget` from the same React entry point as `ParticipantApp`. In the example above, replace the app's render callback with:

```tsx
<ParticipantApp<GameObservation, GameAction, GameCompletion>
  communicationPlacement="game"
  renderGame={(snapshot) => (
    <main>
      <GameView snapshot={snapshot} />
      <aside className="game-dialogue"><CommunicationWidget /></aside>
    </main>
  )}
/>
```

Render exactly one widget in this layout. Style it with scoped CSS, for example `.game-dialogue .parlando-communication { --parlando-communication-radius: 0.25rem; }`. Preserve keyboard focus, form labels, microphone status, and mute feedback. Pilot the same game in both modes when changing communication placement or styling.

Use relative asset URLs in the production build so the package works under
an experiment path on the shared host. A browser pilot can check both role
views, action gating, and the terminal surface with two isolated profiles.
Exercise optional SDK features when the change affects them.
