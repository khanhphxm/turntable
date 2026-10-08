# turntable's librespot-core patch

Based on librespot-core 0.8.0 (MIT). Registry sources and build script are retained; the registry-only integration test target is omitted.

Session changes in `src/session.rs` let turntable own the Premium requirement:

- Remove `check_catalogue` and its calls. Non-Premium attributes are stored normally instead of terminating the entire process with `exit(1)`. The application can stop its engine, remove credentials/account caches and explain the rejection.
- Add `Session::has_product_info()`, backed by an atomic flag set only after a complete ProductInfo packet's attributes are stored. A missing/unrecognised plan is not evidence of a Free account and never triggers credential deletion.
- Re-export the existing `AuthenticationError` from `src/lib.rs`, so the application can match Spotify's `PremiumAccountRequired` response without string matching.
- Remove the now-unfulfilled `#[expect(deprecated)]` on `Credentials::with_blob` in `src/authentication.rs`; no authentication behavior changes.

The workspace patches crates.io to this copy. turntable's `engine::SignInError` and application lifecycle enforce the gate; this patch itself neither authorizes playback nor starts it.

The engine gates both browser sign-in and saved-credential startup before publishing its session to the controller. Spotify can supply the plan in `type` instead of `product`; a Premium `type` with absent or whitespace/container `product` is accepted. Family, Duo and Student are accepted when Spotify reports Premium. Only an explicit `free`/`open` product or type, or Spotify's `PremiumAccountRequired` authentication response, becomes a typed refusal. Missing/unrecognised completed attributes and an unconfirmed subscription at timeout/disconnection are not rejections; ordinary network failures remain retryable and never delete credentials. The decision logs `type`, `product` and ProductInfo completion for diagnosis.
Enable `TURNTABLE_TRACE=/path/to/trace.jsonl` to record the `subscription` mark, including raw `type`/`product`, `has_product_info`, and the acceptance outcome. Timeout/disconnected acceptance is marked `unconfirmed=true`. No account name or credential is logged by this mark.

The controller handles a refusal as sign-out: stop Spirc/audio and cancel session reads, detach the library session, remove saved credentials and account-scoped library snapshot/playlist-index caches, and return to welcome. `State::notice` contains the explanation, while `State::sign_in_error: Option<engine::SignInError>` persists independently of unrelated queued events. Render `error.message()` and offer an Upgrade action via `Msg::OpenUrl(error.upgrade_url().into())`; the URL is `https://www.spotify.com/premium/`. A new sign-in clears the error. Display name, session account ID or saved credential username is included when available.
