# Design

## Context

Motivation, scope and the resolved decisions: `proposal.md`. Observed state of the code:

- `crates/yandex-proto/src/media.rs`: `MediaTarget::{Avatar, Thumbnail}`; `MediaClient::fetch` does a GET with the cookie header only when `cookie_header(cookies, host)` is non-empty, maps 401 to `SessionExpired`, 404 to `NotFound`, other statuses through `http_error`, and collects the body into a `Vec<u8>` capped at `MAX_BYTES` = 25 MiB. Ids go through `safe_path` (per-segment check, so `Id2` = `<bucket>/<uuid>` keeps its `/`). The client is built by `net::http_client` with `reqwest::ClientBuilder::timeout(request_timeout)` (30 s by default), which is a deadline for the whole exchange including the body: a large file would be cut off. Requests are logged through `redact_url` (host and path only).
- `crates/yandex-proto/src/wire.rs`: `WireFile { id (Id2), name, size }`, `WireImage { file, width, height, animated }`, `WireBody::Gallery { items: Vec<WireImage>, text }` are already parsed and fixture-tested (`tests/wire_fixtures.rs`). Nothing to change there.
- `crates/chat-core`: `MediaKind::{Avatar, Thumbnail, File}` already exists (`File` is unused). `Engine::fetch_media` (`client.rs`) serves previews: cache lookup in `media_dir`, `media_missing` TTL, at most `MEDIA_SLOTS` = 4 concurrent fetches, bytes buffered then written by `store_media` (`.part` file, `rename`). Engine tasks are `tokio::spawn`ed on a current-thread runtime and report back with `post(&inputs, Input::…)`; `expire()` ends the session. `ClientConfig` carries `runtime_dir`; `paths.rs` has `escape_component` and `media_dir`. `cache.rs` persists messages as JSON (`MessageBody` is `Serialize`/`Deserialize`) and a `media` table keyed by (kind, id).
- `crates/desktop/src/yandex_backend.rs`: `fetch_media` rejects `MediaKind::File`; `body()` turns a wire gallery into its first image; `preview()` lists the bodies for chat previews. `store.rs::want_media` skips `MediaKind::File`, `want_message_media` requests thumbnails for `MessageBody::Image` only. `timeline.rs::body()` renders `Image` as `thumbnail(...)` (box at the image's aspect ratio, `THUMB_MAX` 320) and `File` as `file_card(...)` (no action). `desktop.rs::Desktop` owns overlays (`menu`, `delete_pending`, `log`) and `blocked()`, which stops the composer and read-marking while one is open. `main.rs::open_chat_window` builds `ClientConfig::new(runtime.path(), …)`.
- Specs: `message-history` "Attachments preview" (to be modified); `message-sending` "Writes only from user actions" (a download is a read, nothing to change); `diagnostic-log` "No secrets in the log" and "Network trace"; `offline-cache` (the cache is under `.runtime/`, downloads are not part of it).
- Reference facts used (research §1 and reference §17.9, captured live by the reference project): file URL `https://files.messenger.yandex.ru/file_shortterm/{fileId}?attach=true`; `fileId` contains `/` and is encoded per segment; the answer is a `302` to a storage host that must be followed; `FileInfo.Name` is the authoritative name, `Content-Disposition` is not.

## Goals / Non-Goals

**Goals:**
- One explicit, cancellable, atomic download path from the UI to disk that never buffers an original in memory and never shares code paths with previews.
- Gallery kept whole in the model and shown as grid plus viewer.
- Work splits into tasks over disjoint files once the shared contract (Decision 3) is fixed.

**Non-Goals:**
- Anything in the proposal's Non-goals; additionally: resuming a partial download (a retry starts over), checksum verification, de-duplicating by content, reading `Content-Disposition` for a name, showing the original (rather than the preview) in the viewer, zoom and pan in the viewer, a global downloads list.

## Decisions

### 1. Originals are a different kind of request from previews
`MediaKind::File` (already in the model) means "original file"; it is never fetched through `Backend::fetch_media`. `YandexBackend::fetch_media` keeps rejecting it, `ChatStore::want_media` keeps skipping it, and `Engine::fetch_media` ignores it with a `warn` (a bug guard, not a code path). An image's original handle is `MediaRef { id: <same Id2>, kind: File }`, built by `MediaRef::original()` from its `Thumbnail` ref. Alternative (a new `MediaKind::Original`) rejected: `File` already exists, is cache-keyed as `"file"`, and has no other meaning.

### 2. Proto: `MediaTarget::Original` and a streaming client
- `MediaTarget::Original { file_id }` → `{files_base}/file_shortterm/{safe_path(file_id)}?attach=true`. Same `safe_path`, so the hostile-id tests apply unchanged.
- `MediaClient::download(&self, target, out: &mut (dyn Write + Send), progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> Result<u64, ProtoError>`: GET as `fetch` does (same cookie rule, same status mapping, same `media exchange` debug record through `redact_url`), then `response.chunk()` loop writing each chunk to `out` and calling `progress(received, content_length)`. No size cap on this path (`MAX_BYTES` stays in `fetch`). When `Content-Length` is known and the body ends with a different count, return `ProtoError::Network("response ended early")`. A write error becomes the new `ProtoError::Storage(String)` ("local write failed"); it carries the `io::Error` kind text only.
- Timeout: `MediaClient` gets a second `reqwest::Client` built by a new `net::http_client_streaming`: `connect_timeout` plus `read_timeout(request_timeout)` (idle time between chunks) and no total timeout. `fetch` keeps the existing client. Alternative (raising `request_timeout`) rejected: it would weaken the "silent server surfaces as an error" guarantee for every other request.
- Redirects: reqwest's default policy (follow up to 10) is kept because the reference observed a `302` to the storage host. reqwest drops the `Cookie` header when a redirect changes host or port; a test with two local stub servers (different ports) pins this, because the spec requires it.
- `Client::download_media(&self, target, out, progress)` is the facade, next to `fetch_media`.
- Implementation notes (1.1): `fetch` and `download` share one private `MediaClient::get` (cookie rule, `media exchange` record, status mapping). The record gains a `landed` field with the host and path the redirects ended on (redacted like `url`), so task 1.3 can show the `302` target without a new return type. `ProtoError::Storage` keeps only the `io::ErrorKind` text. Adding the variant forced an arm in `yandex_backend.rs::backend_error`; it maps to `BackendError::Protocol("local write failed: …")` until task 3.1 replaces it with `BackendError::Storage`.
- A blocking `std::io::Write` inside the async loop: chunks are at most tens of KiB and `store_media` already writes synchronously on the engine thread, so this is the existing practice, not a new cost. Alternative (`AsyncWrite`) rejected: `chat-core` enables no tokio `io`/`fs` features.

### 3. Domain contract in `chat-core` (fixed up front)
```
MediaRef::original(&self) -> MediaRef                       // same id, kind File
GalleryItem { media: MediaRef /*Thumbnail*/, width: u32, height: u32, name: Option<String> }
MessageBody::Image   { media, width, height, caption, #[serde(default)] name: Option<String> }
MessageBody::Gallery { items: Vec<GalleryItem>, caption: Option<String> }
MessageBody::gallery_items(&self) -> Vec<GalleryItem>        // Image -> one item, Gallery -> all, else empty
enum DownloadFailure { Offline, NotFound, Destination, Disk, Network, Other }
Command::Download { media: MediaRef /*kind File*/, name: Option<String> }
Command::CancelDownload(MediaRef)
ClientEvent::DownloadProgress { media, received: u64, total: Option<u64> }
ClientEvent::Downloaded { media, path: PathBuf }              // final path
ClientEvent::DownloadFailed { media, reason: DownloadFailure }
ClientEvent::DownloadCancelled(MediaRef)
BackendError::Storage(String)                                 // a local write failed
Backend::download(&self, media: &MediaRef, out: &mut (dyn Write + Send),
                  progress: &(dyn Fn(u64, Option<u64>) + Send + Sync)) -> Result<u64, BackendError>
ClientConfig::downloads_dir: Option<PathBuf>                  // None: no destination known
```
`Backend::download` has no default body, so every implementor decides (`YandexBackend`, the `Fake` in `client.rs` tests). `#[serde(default)]` keeps cached `Image` rows readable; a cached gallery that was stored as its first `Image` stays one image until its chunk is reloaded from the server (the open chat already replaces its newest chunk). `MessageBody` is matched in `client.rs::preview_text`, `composer.rs`, `yandex_backend.rs::preview` and `timeline.rs::body`: a gallery previews like an image (caption, else "Фото"). The names above are the contract between tasks 2.1–4.3; `DownloadFailure` is deliberately closed and small: the UI maps each case to one Russian line.

### 4. Engine: one download, one task
State in `Engine`: `downloads: HashMap<MediaRef, JoinHandle<()>>` and `download_slots: Semaphore(3)`, separate from `media_slots` so a large file never starves thumbnails. `Command::Download { media, name }`:
1. `media.kind != File` → `warn`, ignore. Already in `downloads` → ignore (one transfer per file id, however many messages show it). `Phase::Expired` → ignore.
2. Not `Phase::Online` → `DownloadFailed { Offline }` at once. Offline is refused, not queued: this is an explicit user action, and a hidden queue would start a transfer minutes later (spec "Offline").
3. `downloads_dir` is `None` or not a directory → `DownloadFailed { Destination }`.
4. Spawn: acquire a slot (waiting shows as zero progress), `paths::create_partial(dir, safe_name)` → `<name>.part` created with `create_new` inside the Downloads folder (numeric suffix on the partial name when taken, so two downloads of the same name never share a partial), `backend.download(&media, &mut file, &progress)`, flush, `paths::commit_partial` → `hard_link(partial, final)` which fails if `final` exists (atomic no-overwrite; on `AlreadyExists` try `name (1).ext`, `(2)`, …), then remove the partial. Result is posted as `Input::Download { media, result }`.
5. A `Partial` guard owns the partial path and removes the file in `Drop` unless committed. Cancelling, an error, aborting the task and a panic therefore all clean up without separate code. A crash (kill -9) may leave `<name>.part` in the Downloads folder; the final name is never touched (spec "Complete or absent").
6. Progress: the `progress` closure posts `Input::DownloadProgress` at most every 100 ms and once at the end, so the unbounded event channel does not fill with per-chunk events.
7. `Command::CancelDownload(media)`: remove the handle, `abort()`, emit `DownloadCancelled`. A result that arrives after the entry is gone is still reported as it happened (`Downloaded` if the file was linked): the store treats events as authoritative and the last one wins.
8. Result mapping: `SessionExpired` → `self.expire()` (which, like sign-out, aborts every handle in `downloads`, so partials are removed); `NotFound` → `NotFound`; `Storage(_)` → `Disk`; `Network`/`Protocol` → `Network`/`Other`, logged at `warn` with the error text, without the file name. Name sanitation and collision handling are pure functions in `paths.rs` (Decision 5), not engine logic.

Completed downloads are not recorded in the cache and are not touched by sign-out; only in-flight ones are cancelled.

### 5. Destination and names: `paths.rs`, pure and tested
`sanitize_file_name(name: Option<&str>) -> String`: take the last path component after `/` and `\`; replace control characters and `:` with `_`; trim whitespace and leading dots; shorten to 200 bytes on a `char` boundary keeping the extension (up to 16 bytes); empty, `.`, `..` or `None` → `"Файл"`. `unique_name(stem_ext, n)` produces `report (1).pdf`. `downloads_dir(home: Option<OsString>) -> Option<PathBuf>` is `$HOME/Downloads` (no new dependency; the app is macOS-only), computed in `main.rs` and passed through `ClientConfig::downloads_dir`; a missing `HOME` makes every download fail with `Destination`. Alternatives rejected: a native folder API (new dependency or objc2 calls for the one value that `$HOME/Downloads` gives), writing under `.runtime/` (not where the user looks, and cleaned with the account, which the proposal rejects).

`hard_link` needs a filesystem that supports it (APFS does). If it fails with anything but `AlreadyExists`, the download fails with `Disk` and the partial is removed; no fallback to `rename`, which could overwrite.

### 6. Store: download state beside, not inside, the media map
`ChatStore` gets `downloads: HashMap<MediaRef, DownloadState>` with `DownloadState::{Running { received, total }, Saved(PathBuf), Failed(DownloadFailure)}`, keyed by the original ref. `download(original, name)` writes `Running { 0, None }` first (the card reacts at once) and sends `Command::Download`; if the connection is not `Online` it writes `Failed(Offline)` and sends nothing (belt and braces: the UI already disables the action). `cancel_download` sends `CancelDownload` and waits for `DownloadCancelled` to clear the state. The events above update the map and emit `StoreEvent::Download(MediaRef)`; `SignedOut` clears it. The thumbnail maps `media` and `media_requested` are untouched, so downloads can never be mistaken for previews. `want_message_media` requests every `GalleryItem` thumbnail (bounded by the 4 preview slots).

### 7. Timeline: card states and grid
File card (`timeline.rs`): below name and size one action line whose text follows the state: idle "Скачать"; offline "Нет соединения" (disabled); running "`<received> из <total>`" (or just received) with "Отмена"; saved "Сохранено: `<final name>`" with "Скачать ещё раз"; failed "`<reason>`" with "Повторить". Reasons: Offline "Нет соединения", NotFound "Файл больше недоступен", Destination "Папка «Загрузки» недоступна", Disk "Не удалось записать на диск", Network "Ошибка сети", Other "Не удалось скачать". Sizes reuse `human_size`.

Gallery grid: a pure function `gallery_columns(n) -> usize` (1 for one image, 2 for two or four, otherwise 3) and square cells cropped with `ObjectFit::Cover`, widths filling `BUBBLE_MAX - 2 * BUBBLE_PAD_X` shared by columns with a small gap; the last row is left-aligned; one image keeps the `thumbnail()` box at its aspect ratio. Each cell keeps the placeholder-until-loaded rule and the `debug_selector` pattern (`msg-{ts}-image-{index}`, `-loaded`). Clicking a cell or the single image calls `Desktop::open_viewer(chat, ts, index)` through `RowContext::desktop`. More than ten items are not expected; the grid does not paginate.

### 8. Viewer: an overlay owned by `Desktop`
A new module `crates/desktop/src/viewer.rs` holds `Viewer { chat, ts, index }`, the pure `step(index, len, delta) -> usize` (clamped, no wrap) and `render(...)`. `Desktop` gets `viewer: Option<Viewer>`; `open_viewer`/`close_viewer`; `blocked()` includes it, so the existing `place_focus` and read-marking rules apply with no new code (it takes focus off the composer, so the `TextInput` left/right bindings do not fire). Items are read from the store on every render through `MessageBody::gallery_items`, so the viewer follows live edits; when the message or its images disappear (`TimelineChange::Removed` or a deleted body) it closes. The shown image is the cached preview (the same file the grid uses, no extra request); until it arrives a placeholder at the image's aspect ratio is shown. Escape reuses the existing `Dismiss` action (`desktop.rs::dismiss` closes the viewer first); `ViewerPrev` and `ViewerNext` are new actions bound to `left`/`right` in `main.rs` under the `BlazingTelesram` context next to `escape`, and in the `tests.rs` harness. The viewer's Download button reads and writes the same `DownloadState` as the card for `item.media.original()` and passes `item.name`.

### 9. Logging and privacy
The download request logs through the existing `media exchange` record (host and path, status, elapsed; no query because of `redact_url`). The engine logs `info`/`warn` with byte counts and error text only; file names and cookie values appear in no record above `trace` (spec "Authenticated and private transfer"); a test with the capturing logger asserts it for a successful and a failed download. Saved files are never opened, previewed by the OS or indexed by the app.

## Risks / Trade-offs

- **URL template and `Id2` handle were verified live for Disk images only** (section "Live verification") → task 1.3 repeats the check with the Rust client on a large non-image file; the template lives in one function (`MediaTarget::url`), so a mismatch is a one-line fix plus a design note before tasks 3.1 and 5.x finish.
- **Redirect chain of two hops** (`files.messenger.yandex.ru` → `downloader.disk.yandex.ru` → `mds.storage.yandex.net`) → reqwest's default limit of 10 covers it; the signed storage URLs are short-lived, so the client never stores or logs a redirect target (and `redact_url` already drops the query).
- **No size cap** → a huge file fills the disk until cancelled (proposal Decision 4); mitigated by progress, Cancel and removal of the partial on `Disk` errors. Only three transfers run at once.
- **Orphan `<name>.part` after a crash** → visible in Downloads, never under the final name; accepted, not cleaned at startup because the app cannot tell its own leftovers from the user's files.
- **`hard_link` unsupported on the Downloads volume** → the download fails visibly with a disk reason instead of risking an overwrite.
- **Cached gallery messages show one image until reloaded** (Decision 3) → bounded: the newest chunk of an opened chat is replaced from the server; older ones on the next scroll load.
- **Concurrent changes touch the same files** (`model.rs`, `backend.rs`, `client.rs`, `yandex_backend.rs`, `store.rs`, `timeline.rs`, `desktop.rs`, `tests.rs`, `bt-probe.rs` are also candidates for `add-message-reactions`, `add-global-search`, `add-chat-members-and-creation`). Edits here are additive (new variants, new trait method, new module `viewer.rs`); tasks rebase before editing and keep diffs minimal. New tests are appended to `tests.rs` in one delimited section.
- **Blocking writes on the engine thread** → same as `store_media` today; chunk-sized and local. If the Downloads folder is on a slow network volume the engine thread stalls between chunks; accepted for a single-user desktop client.

## Live verification

2026-11-24, before the specs were finalized. Method: read-only GETs with the stored session of the signed-in account (cookie attached only for `files.messenger.yandex.ru`, never forwarded on redirects), `bt-probe history` to find messages; no file was saved, bodies were only counted. Private content (ids, names, chat) is deliberately not recorded here.

| Check | Result |
|---|---|
| `GET https://files.messenger.yandex.ru/file_shortterm/{Id2}?attach=true`, `Id2` = `disk/<uuid>` (`Source` Disk) | `302`, empty body, no `Content-Disposition` |
| first redirect target | `downloader.disk.yandex.ru`, `302`, `Content-Disposition` present, no cookie sent and none needed |
| second redirect target | `mds.storage.yandex.net`, `200`, `Content-Length` announced, `Content-Disposition: attachment`, correct `Content-Type` (`image/jpeg`, `image/png`) |
| body size vs `FileInfo.Size` | equal for both images tested (62962 and 240897 bytes) |
| `?size=MIDDLE2048` for the same id | `302` as well (thumbnail path unchanged) |

Consequences: the template of Decision 2 stands; handles are `disk/<uuid>` in practice (the "bucket" is `disk`); the chain has two redirect hops; `Content-Length` is announced, so the truncated-body check and the total in progress work. The self chat holds no files; any chat works for `bt-probe download`. The two remaining checks (a file above 25 MiB through the Rust client, a non-image `MiscFile`) are recorded below.

### Re-check with the Rust client (task 1.3)

2026-11-25. Method: `bt-probe download <chat> <micros> [--item N] --to <temporary directory>` (`Client::download_media`, the code path of the app) with the stored session (`BT_RUNTIME_DIR` of the main checkout), read-only against the real account, in the group chat that holds the files; `BT_LOG=yandex_proto=debug` for the redirect target. The binary was the debug `bt-probe` already built in the main checkout (its `media.rs`, `net.rs` and `client.rs` are identical to this branch): a freshly built `bt-probe` from a new path never got a connection through the host's per-application firewall (TCP connects to Yandex timed out), the allowed path did. Every downloaded file was deleted afterwards and no ids, names or chats are recorded here.

| Check | Result |
|---|---|
| (a) `MiscFile` (`WireBody::File`, a video), `FileInfo.Size` 60344549 bytes (about 57.5 MiB, above the 25 MiB cap) | saved in full: `received` 60344549, equal to `FileInfo.Size` (`size check: matches`); file on disk 60344549 bytes |
| (a) progress total | the last `progress` call carried `total` 60344549 = the announced `Content-Length` |
| (a) 25 MiB cap | not applied: the original path streamed 2.3 times the cap to disk without a refusal |
| (a) redirect | request `files.messenger.yandex.ru/file_shortterm/disk/<uuid>`, final host `mds.storage.yandex.net` (the `landed` field of the `media exchange` record); the record carries no query |
| (b) single `Image` message, 184397 bytes | `received` 184397 = `FileInfo.Size` = `Content-Length`, `size check: matches` |
| (b) second image of a three-image `Gallery` (`--item 2`), 336932 bytes | `received` 336932 = `FileInfo.Size` = `Content-Length`, `size check: matches` |

Consequences: no result contradicts Decision 2; `MediaTarget::url` and the `Original` template stand unchanged, so no code change was needed. Streaming, the idle (read) timeout and the status mapping held for a 57.5 MiB body; all three runs exited with status 0. One observation: the debug build of `bt-probe` needed about 90 s for the 57.5 MiB file (about 70 s of CPU, TLS in a debug profile), so that time says nothing about the release build of the app; the idle timeout never fired. The larger files of the group chat (up to about 265 MB) were not downloaded.
