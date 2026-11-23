# PostHog AI Observability setup

## Status

**Wired, unverified in PostHog.** The production renderer build succeeds, but no live LLM request was made during setup because it requires a user's OpenAI or ChatGPT-plan credentials.

## Integration selected

- **Workflow:** `ai-observability-manual-capture`
- **Why:** The Node project manifest contains no vendor LLM SDK. The app makes realtime OpenAI calls directly through WebRTC and native Tauri commands, so automatic SDK wrapping is not available.
- **Package:** Added `posthog-node` `^6.0.5` as the Node manual-capture dependency.
- **Transport:** The renderer uses a small manual capture helper so capture remains browser-safe at the actual realtime call site.

## What changed

- Added `src/lib/posthogAi.ts`.
  - Reads `VITE_POSTHOG_KEY` and `VITE_POSTHOG_HOST` at build time; no PostHog credentials are hardcoded in source.
  - Sends non-blocking `$ai_generation` and `$ai_span` events to PostHog's capture endpoint.
  - Emits a development console error if either environment variable is missing, while production remains a no-op.
- Updated `src/features/voice/live/session.ts`.
  - Creates one `$ai_session_id` for each live-voice connection.
  - Creates a new `$ai_trace_id` at the start of each spoken turn and reuses it for all realtime generations and tool spans in that turn.
  - Captures API-key realtime responses and ChatGPT-plan voice responses as OpenAI generations, including available transcripts and Realtime usage counts.
  - Captures every completed live-voice tool run as an `$ai_span`, sharing its turn trace and session IDs.
  - Leaves events anonymous because the live-voice code has no authenticated user ID in scope.
- Added the configured browser-safe environment variables to `.env`.
- Added `.posthog-wizard-cache/.posthog-ai.json` for the wizard report pipeline.

No existing PostHog initialization, identity calls, event capture, or dashboards were changed.

## Verification performed

- `pnpm build` completed successfully.
- Confirmed both required browser environment variables are present in `.env`.

## Verify in PostHog

1. Start **Live voice** with either the API-key or ChatGPT-plan path.
2. Speak a request that invokes a tool, for example ask it to list the current panes.
3. Make a second request in the same live-voice conversation.
4. In **AI Observability → Traces**, open the newest trace and confirm:
   - both turns share one AI session;
   - each spoken turn has its own trace;
   - a tool-using turn includes a tool span;
   - the generation provider is `openai`; and
   - the events are anonymous, as designed.

## Privacy mode

This manual-capture implementation currently sends available voice transcripts in `$ai_input` and `$ai_output_choices`; this is effectively **privacy mode off**. It is controlled by the payload construction in `src/features/voice/live/session.ts`, not by an SDK `privacyMode` setting.

Before collecting prompts or responses that must not be stored in PostHog, remove or redact those two properties at the manual capture call sites. Doing so prevents future manual events from carrying that content; it does not alter existing events or arbitrary custom properties. See [AI Observability privacy mode](https://posthog.com/docs/ai-observability/privacy-mode) for the behavior of SDK-managed privacy mode.
