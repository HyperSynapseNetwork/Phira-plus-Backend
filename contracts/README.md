# PPB HTTP Contract

`contracts/openapi.json` is the frozen HTTP contract consumed by PPF and Panel. Runtime source of truth is PPB's `utoipa` route/schema declarations.

Provider-side exposure is separately frozen: `contracts/route-surface.json` classifies every runtime HTTP method/path as a frozen REST operation or an explicit browser/realtime/infrastructure exception. `contracts/realtime.json` gives SSE/WebSocket channels typed parameter semantics; in particular Live uses `/ws/v1/rooms/{room_id}/live` with `room_id = pmp_room_id`. Build and Release must run the Route Surface and Realtime gates so hidden legacy endpoints cannot sit outside OpenAPI.

## REST Error Contract v1.1

Non-2xx REST responses use `ErrorEnvelope`. `ErrorBody.code` references the generated `ErrorCode` enum; `message` is diagnostic/legacy fallback and MUST NOT be used as formal UI copy. `request_id` is non-empty and must match `X-Request-ID`. `details.params` is a safe, explicitly consumable parameter map.

The ErrorCode source chain is intentionally one-way:

`PPB error::ErrorCode` → `OpenAPI ErrorCode enum` → `contracts/error-codes.json` / generated TypeScript → PPF + Panel zh/en completeness gates.

Do not maintain a second handwritten code list in this README. Unknown future server codes must be preserved by clients and fall back to localized `UNKNOWN_ERROR`; they must never be remapped to an unrelated known code.

Malformed JSON/query/path, unsupported method/content type, body-size rejection, 404 and panic/500 responses are normalized by the outer request-context middleware into the same envelope. `INTERNAL_ERROR` never exposes DB/fs/crypto/JWT exception text.

System-generated persistent Notifications use semantic `title_key/body_key/action.label_key + params`; administrator-composed free text remains literal. PPNotice is a separate client-only short-lived feedback layer and is not part of Notification Hub persistence.
