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

Use relative asset URLs in the production build so the package works under
an experiment path on the shared host. A browser pilot can check both role
views, action gating, and the terminal surface with two isolated profiles.
Exercise optional SDK features when the change affects them.
