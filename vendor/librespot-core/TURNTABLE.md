# turntable's librespot-core patch

Based on librespot-core 0.8.0 (MIT). Registry sources and build script are retained; the registry-only integration test target is omitted.

Session changes in `src/session.rs` let turntable own the Premium requirement:

- Remove `check_catalogue` and its calls. Non-Premium attributes are stored normally instead of terminating the entire process with `exit(1)`. The application can stop its engine, remove credentials/account caches and explain the rejection.
- Add `Session::has_product_info()`, backed by an atomic flag set only after a complete ProductInfo packet's attributes are stored. A missing/unrecognised plan is not evidence of a Free account and never triggers credential deletion.
- Re-export the existing `AuthenticationError` from `src/lib.rs`, so the application can match Spotify's `PremiumAccountRequired` response without string matching.
- Remove the now-unfulfilled `#[expect(deprecated)]` on `Credentials::with_blob` in `src/authentication.rs`; no authentication behavior changes.
- Add an optional `Cache::set_credentials_writer` host hook. turntable serializes atomic/private credential saves with a cancellable account-start authorization; sign-out cancels it before removing credentials, so unfinished OAuth/Spirc authentication cannot recreate the sign-in or overwrite a newer account.
- Support HTTP CONNECT proxy Basic authentication in the native AP/dealer tunnel and shared HTTP client. Force CONNECT for proxied HTTP requests so proxy credentials reach only the proxy, remove URL credentials from the connector address, include the CONNECT Host header, and never log the proxy URL (which can contain a password).

The workspace patches crates.io to this copy. turntable's `engine::SignInError` and application lifecycle enforce the gate; this patch itself neither authorizes playback nor starts it.

The engine gates both browser sign-in and saved-credential startup before publishing its session to the controller. Spotify can supply the plan in `type` instead of `product`; a Premium `type` with absent or whitespace/container `product` is accepted. Family, Duo and Student are accepted when Spotify reports Premium. Only an explicit `free`/`open` product or type, or Spotify's `PremiumAccountRequired` authentication response, becomes a typed refusal. Missing/unrecognised completed attributes and an unconfirmed subscription at timeout/disconnection are not rejections; ordinary network failures remain retryable and never delete credentials. The decision logs `type`, `product` and ProductInfo completion for diagnosis.
Enable `TURNTABLE_TRACE=/path/to/trace.jsonl` to record the `subscription` mark, including raw `type`/`product`, `has_product_info`, and the acceptance outcome. Timeout/disconnected acceptance is marked `unconfirmed=true`. No account name or credential is logged by this mark.

The controller handles a refusal as sign-out: stop Spirc/audio and cancel session reads, detach the library session, remove saved credentials and account-scoped library snapshot/playlist-index caches, and return to welcome. `State::notice` contains the explanation, while `State::sign_in_error: Option<engine::SignInError>` persists independently of unrelated queued events. Render `error.message()` and offer an Upgrade action via `Msg::OpenUrl(error.upgrade_url().into())`; the URL is `https://www.spotify.com/premium/`. A new sign-in clears the error. Display name, session account ID or saved credential username is included when available.

Sign-out and Premium rejection share one account teardown. All account-scoped state is reset immediately while device preferences remain. Library requests, startup/player events and lyrics carry an account generation; stale queued/in-flight work cannot restore UI data or re-enable snapshot persistence. Startup authorization cancellation also guards the native credential cache write itself, rather than merely discarding the eventual engine response.

`build.rs` no longer runs vergen or draws a random build ID. Upstream embedded the enclosing repository's git commit and dates, which made every commit rebuild librespot-core and its dependents (locally and in CI). The informational strings now use the crate version: the AP login's system information string reads `librespot-0.8.0-turntable`, and `VERSION_STRING` reads `librespot-0.8.0`. The `vergen-gitcl`, `rand` and `rand_distr` build-dependencies are removed.

`src/dealer/mod.rs` starts tungstenite's read buffer at 4 KiB rather than 128 KiB
for small dealer JSON/control messages. It still grows for large messages;
message/frame/write limits and TLS/proxy/reconnection behavior are unchanged.
This applies across platforms; tests cover large whole and fragmented messages.
