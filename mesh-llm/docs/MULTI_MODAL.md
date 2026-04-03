# Multi-Modal Roadmap

## Status

Design proposal.

Current PR scope is the `chat/completions` vertical slice plus blob/object transport and console attachment support.

Next planned build after this PR:

- `POST /v1/responses`

Explicitly deferred beyond this PR:

- `POST /v1/audio/transcriptions`
- `POST /v1/audio/speech`
- `v1/realtime`

mesh-llm already has part of the foundation:

- vision capability inference
- `mmproj` launch support for vision-capable llama.cpp models
- image attachment support in the web console
- request forwarding to llama.cpp `/v1/chat/completions`

What is still missing is a complete mesh-llm multimodal contract: capability advertisement, routing, request normalization, blob transport, and console support for more than images.

## Goals

- Support multimodal inference through mesh-llm using llama.cpp-compatible models.
- Route image and audio requests to compatible hosts automatically.
- Keep media transport bounded and private by default.
- Preserve protocol compatibility unless we explicitly choose a breaking change.
- Keep the crate/plugin split clean: core owns inference routing, plugin owns media object storage.

## Non-Goals

- Permanent distributed file storage
- IPFS/libp2p-first design
- Voice-to-voice parity with OpenAI Realtime on day one
- Audio generation from llama alone
- Native end-to-end video inference on the current llama.cpp path

## API Targets

### Phase 1

- `POST /v1/chat/completions`
- `GET /v1/models`

This is the shortest path because llama.cpp already supports multimodal chat here for supported models.

### Phase 2

- `POST /v1/responses`

Implement as a mesh-llm compatibility shim after chat completions are solid.

### Optional Later

- `POST /v1/audio/transcriptions`
- `POST /v1/audio/speech`
- `v1/realtime`

These should be treated as separate product surfaces, not prerequisites for multimodal chat.

## Capability Model

Do not collapse everything into `vision`.

Recommended model capabilities:

- `multimodal: bool`
- `vision: CapabilityLevel`
- `audio: CapabilityLevel`
- `reasoning: CapabilityLevel`
- `tool_use: CapabilityLevel`
- `moe: bool`

Why:

- `multimodal` is a useful umbrella signal for UI and coarse filtering.
- `vision` and `audio` are still required for correct routing.
- A model can be multimodal without supporting both image and audio equally.

## Protocol Plan

Preferred path: additive change.

- add `multimodal`
- add `audio`
- keep `vision` meaning image/vision support

Repurposing `vision` to mean generic multimodal would be a breaking semantic change:

- old nodes would misinterpret the flag
- routing and UI would become incorrect in mixed-version meshes
- protobuf only protects unknown fields, not changed meaning of existing fields

If we ever want to make that breaking change anyway, it should be an explicit protocol-version decision.

## Request Formats

### Chat Completions

Accept and preserve OpenAI-style content parts:

- text
- image URL / data URL
- audio URL / data URL
- future file-style references

Video is not a Phase 1 request shape target. Treat video separately from image/audio until the serving stack can handle it natively.

mesh-llm should detect multimodal intent from structured content blocks, not just text keywords.

### Responses API

Add a translation layer from `responses` input items into chat-completions-style message content after Phase 1 is working.

## Routing Work Required

- Add `audio` and `multimodal` to model capability inference.
- Extend local and remote model metadata detection for audio-capable model families.
- Advertise audio/multimodal capabilities in `/api/status` and `/v1/models`.
- Route multimodal payloads by structured content inspection, not only prompt text.
- Add audio-aware routing heuristics similar to current image-aware routing.
- When `model=auto`, prefer:
  - vision-capable hosts for image inputs
  - audio-capable hosts for audio inputs
  - models supporting both when both are present
- Skip or constrain the pre-plan pipeline for requests containing media until the planner path is multimodal-safe.

## Video Support

Some model families support video at the model level, but that should not be treated as available in mesh-llm yet.

Current working assumption:

- open multimodal models with video support exist
- the current llama.cpp integration path in mesh-llm should be treated as image/audio only
- native video input should stay out of scope until upstream serving support is real and reliable

Recommended first implementation path for video:

- accept uploaded video into the same request-scoped blob plugin
- decode and sample frames server-side
- send sampled frames as ordered image inputs to a vision-capable model
- optionally include timestamp metadata in the prompt or content structure

Why this path first:

- it reuses the existing blob/object lifecycle
- it works with current image-capable serving paths
- it avoids blocking on native video support in llama.cpp

Follow-up requirements when we do video:

- add `video` capability metadata only when there is a real serving path behind it
- define upload limits, codec/container acceptance, and frame-sampling defaults
- decide whether video should be exposed only through `responses`, through `chat/completions`, or both
- make it explicit in the UI when video is being converted into sampled images rather than handled natively

## Media Transport Plan

Large media should not travel inside request JSON by default.

### First Pass

Use a request-scoped mesh content store implemented as a plugin.

Properties:

- ingress node stores the uploaded object locally
- no replication
- client receives an opaque secret token, not a content hash
- completion request references the token
- serving node fetches the object from the ingress node if needed
- ingress node deletes the object when the request reaches terminal state
- short TTL cleanup handles crashes, disconnects, and abandoned requests

### External vs Internal Identity

- external object ID: secret high-entropy token
- internal storage key: content hash for integrity and dedupe only

The token should be user-visible and fetch-authorizing. The content hash should remain internal.

## Blob Plugin Shape

The blob/content store should be a plugin, not core storage logic.

Core responsibilities:

- parse inference requests
- choose inference targets
- decide request lifecycle
- tell the plugin when a request starts and ends

Plugin responsibilities:

- store request-scoped media on ingress
- mint and validate opaque tokens
- serve media fetches to remote hosts
- enforce TTL and cleanup

Initial plugin operations:

- `put_request_object`
- `get_request_object`
- `complete_request`
- `abort_request`
- `reap_expired_objects`

## Object Lifecycle

### Request-Scoped Media

1. Client uploads image/audio/file to the ingress node.
2. Plugin stores bytes locally and returns a short-lived secret token.
3. Client sends completion request referencing that token.
4. Chosen host fetches from ingress if the object is not local.
5. Request completes, fails definitively, or is canceled.
6. Ingress plugin deletes token and blob after a short grace window.
7. TTL fallback cleans up leaks.

Recommended first-pass behavior:

- no replication
- token bound to one request
- allow a small retry budget for reroute / transport retries
- keep only a short grace period after terminal state

## Console Work Required

### Existing

- image attach UI exists already

### Needed

- add audio attachment UI alongside the existing image flow
- optionally add generic file attachment UI for future file-style message parts
- upload request-scoped objects before sending the completion request
- replace large inline audio/file payloads with secret token references
- keep small inline images working where practical, but support the same upload path when needed
- show attachment previews / badges before send:
  - image thumbnail
  - audio filename, duration if available, and remove action
  - generic file name, size, mime type, and remove action
- show upload state clearly:
  - pending
  - uploaded
  - failed
  - retrying
- block send or degrade gracefully if required uploads fail
- allow `model=auto` to switch to multimodal-capable models when attachments are present
- show capability hints in the model picker:
  - vision
  - audio
  - multimodal
- preserve attachment state through request retries and host reroutes for the same request
- support cancel/removal of pending uploads before dispatch
- add a clear fallback UX when no compatible warm model is available
- keep the chat transcript representation explicit about attachment kind instead of flattening everything into plain text

### Suggested Console Sequence

1. User attaches image/audio/file.
2. Console creates a pending attachment entry in local UI state.
3. Console uploads the object to the ingress node and receives a request-scoped token.
4. Pending entry becomes uploaded attachment metadata.
5. Completion request is sent with structured message content referring to the token.
6. If the request is retried, reuse the same token while the request is still alive.
7. When the request finishes, normal cleanup happens on the ingress node.

### Nice-to-Have Later

- drag-and-drop attachments
- paste image support
- microphone capture for audio input
- waveform / duration preview for audio
- video upload once there is either frame-sampling support or native serving support
- richer attachment rendering in the transcript
- attachment reuse inside the same conversation only if we later decide to support longer-lived object leases

## Body Size and Transport Limits

Today the proxy body limits are tuned for JSON chat, not large media.

We should:

- keep small inline image support where it is practical
- prefer upload-and-reference for audio and larger files
- avoid raising global HTTP body limits as the main solution

## Endpoint-Specific Feasibility

### `/v1/audio/transcriptions`

Feasible as a mesh-llm shim.

Implementation approach:

- accept uploaded audio
- route to an audio-capable model
- use a transcription-oriented prompt / template
- return OpenAI-shaped transcription output

Tradeoff:

- this is prompt-based ASR behavior, not a purpose-built transcription stack

### `/v1/audio/speech`

Not a llama-only feature today.

This requires a separate TTS backend if we want to expose the endpoint.

### `v1/realtime`

Possible only as a mesh-llm shim layer at first:

- websocket/event protocol in mesh-llm
- llama.cpp still serving underlying chat completion / multimodal input
- separate STT/TTS components required for full speech-in/speech-out parity

## Implementation Phases

## Recommended Build Order

Build the smallest end-to-end vertical slice first.

### First Build

Make `/v1/chat/completions` handle request-scoped media attachments end to end for an explicitly selected compatible model.

Scope:

- capability plumbing for `multimodal` and `audio`
- media-safe request parsing and pipeline bypass
- request-scoped blob plugin skeleton and core integration points
- console attachment upload path for audio first

Why first:

- it exercises the real request path
- it validates the capability model
- it proves the ingress-storage design before we add more API compatibility layers

### Second Build

Add good `model=auto` behavior.

Scope:

- auto-route image/audio to compatible models
- capability hints in model picker and model listings
- no-compatible-model fallback UX
- retry / reroute behavior for requests with media attachments

### Third Build

Add multimodal `/v1/responses` compatibility.

This is the next planned implementation after the current PR.

### Later

- video upload via frame sampling into image inputs
- `/v1/audio/transcriptions` shim
- `v1/realtime` shim
- `/v1/audio/speech` only with a separate TTS backend

### Phase 1: Capability Plumbing

- add `multimodal` and `audio` capabilities
- update protobuf, status payloads, and model listings
- update model capability inference

### Phase 2: Routing and Request Parsing

- detect multimodal request parts structurally
- add audio-aware routing
- bypass fragile pipeline paths for media requests
- make `auto` select multimodal-capable models

### Phase 3: Blob Plugin

- implement request-scoped ingress storage plugin
- add upload endpoint
- add host fetch path
- add request-finalization cleanup

### Phase 4: Console

- add audio/file uploads
- upload before request dispatch
- send token references
- improve multimodal model selection UX

### Phase 5: Compatibility Shims

- `/v1/responses`
- optional `/v1/audio/transcriptions`
- optional `v1/realtime`

### Phase 6: Optional Extended Backends

- TTS backend for `/v1/audio/speech`
- video ingestion and frame sampling pipeline
- richer persistent asset handling if we ever want reusable attachments

## Open Questions

- exact token format and request binding rules
- whether tiny inline images should remain supported in addition to blob upload
- whether `responses` should become the primary public API once parity is good enough
- whether multimodal requests should always disable pre-plan, or only for audio
- whether plugin-host fetches should flow through the existing mesh transport or a dedicated plugin RPC path
- when to introduce a real `video` capability instead of treating video as a higher-level image sequence workflow
