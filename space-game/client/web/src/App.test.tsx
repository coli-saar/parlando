import { describe, expect, it } from "vitest";
import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { initialVoicePreflight, initialVoiceStatus } from "@coli-saar/parlando-client";
import { ParlandoStartupGate } from "@coli-saar/parlando-client/react";
import { App, CommunicationPanel } from "./App";

describe("Space Game startup", () => {
  it("delegates Parlando setup and waiting-room startup to the SDK gate", () => {
    const element = App() as ReactElement<{ children: ReactElement }>;
    const gate = element.props.children as ReactElement<{
      renderGame: unknown;
    }>;

    expect(element.type).toBe("main");
    expect(gate.type).toBe(ParlandoStartupGate);
    expect(typeof gate.props.renderGame).toBe("function");
  });

  it("keeps the live level visible but ghosted and gives mute precedence in the compact status", () => {
    const markup = renderToStaticMarkup(
      <CommunicationPanel
        chatDraft=""
        conversation={[]}
        voiceEnabled
        onChatDraftChange={() => undefined}
        onSubmitChat={() => undefined}
        onMicrophoneMutedChange={() => undefined}
        voicePreflight={{ ...initialVoicePreflight, micLevel: 0.62, micProbeActive: true }}
        voiceStatus={{ ...initialVoiceStatus, connected: true, microphoneEnabled: false, message: "Microphone muted" }}
      />
    );

    expect(markup).toContain("Microphone");
    expect(markup).toContain("microphone-name-state muted");
    expect(markup).toContain("microphone-status-chip muted");
    expect(markup).toContain("Muted</div>");
    expect(markup).toContain("Unmute mic");
    expect(markup).toContain('aria-pressed="true"');
    expect(markup).toContain("mic-meter muted");
    expect(markup).toContain("scaleX(0.62)");
  });

  it("shows a compact live badge beside the microphone details without repeating readiness", () => {
    const markup = renderToStaticMarkup(
      <CommunicationPanel
        chatDraft=""
        conversation={[]}
        voiceEnabled
        onChatDraftChange={() => undefined}
        onSubmitChat={() => undefined}
        onMicrophoneMutedChange={() => undefined}
        voicePreflight={{ ...initialVoicePreflight, deviceLabel: "Desk microphone", micProbeActive: true }}
        voiceStatus={{
          ...initialVoiceStatus,
          connected: true,
          microphoneEnabled: true,
          transcriptionMessage: "ASR ready"
        }}
      />
    );

    expect(markup).toContain("Desk microphone");
    expect(markup).toContain("microphone-name-state live");
    expect(markup).toContain("microphone-status-chip live");
    expect(markup).toContain("Live</div>");
    expect(markup).not.toContain("ASR ready");
    expect(markup).not.toContain("Microphone live");
  });
});
