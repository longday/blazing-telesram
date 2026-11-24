# Proposal

> Single context document for this change. Full research: [`research.md`](research.md) (roadmap item 2, media). Technical approach: [`design.md`](design.md); behavior: [`specs/`](specs/); work breakdown: [`tasks.md`](tasks.md).

## Why

Attachments are visible but inert: an image shows only a middle-size thumbnail, a gallery collapses to its first image, and a file card cannot be opened or saved. To replace the web client for daily use the user must be able to get the file out of a message and look at every image of a gallery. This is the first of three slices of roadmap item 2 (media); it is read-only, so it does not touch the write-request allowlist introduced by `add-sending`.

## What Changes

- Explicit file download: a "Download" action on the file card saves the original to disk. Nothing is downloaded automatically when history loads (only avatars and image thumbnails, as today).
- Gallery becomes a first-class message body: the application model keeps all images (today `yandex_backend.rs` keeps only the first), the timeline shows them as a grid.
- Image viewer: clicking an image or a grid cell opens a viewer for the message's images with previous/next navigation and a "Download" action for the shown image.
- Media layer: `MediaTarget` gains an original-file target alongside `Avatar` and `Thumbnail`; the download goes through the existing authenticated `MediaClient` GET path (cookie only for the matching host), streamed to disk with progress, cancel and failure states. The 25 MiB cap stays for in-memory previews only; originals have no size cap (Decision 4).
- Offline and failure behavior: a failed or interrupted download leaves no partial file under the final name and can be retried; offline shows the action as unavailable.

Non-goals: uploading and sending images or files (slice 2); voice and video playback, waveform, video streams (slice 3); `get_media_messages` media browser; automatic download of any original; deleting server-side files; drag-and-drop; opening or revealing a saved file from the app; a "Save as…" dialog; remembering across restarts which files were saved.

## Capabilities

### New Capabilities
- `attachment-download`: explicit saving of a message's file or image original to disk: action, destination, overwrite policy, progress, cancel, failure and retry.

### Modified Capabilities
- `message-history`: the "Attachments preview" requirement changes: "Opening or downloading files is out of scope" is removed (moved to `attachment-download`), and galleries show all images in a grid; a new "Image viewer" requirement is added.

## Impact

- `crates/yandex-proto`: `media.rs` (original-file target and URL template, streaming download to a writer instead of a memory buffer, no 25 MiB cap on that path), `net.rs` (a streaming HTTP client with an idle timeout instead of the 30 s total deadline), `client.rs` facade, `bin/bt-probe.rs` (`download` command). `wire.rs` is unchanged: `WireFile` already carries `Id2`, name and size, and the gallery is already parsed.
- `crates/chat-core`: `model.rs` (`MessageBody::Gallery`, `name` on images, download `Command`s and `ClientEvent`s), `backend.rs` (`Backend::download`), `client.rs` (download engine: bounded concurrency, partial file, cancel, sign-out cleanup), `paths.rs` (safe file name, collision-free destination). `cache.rs` is unchanged: originals never enter the thumbnail cache.
- `crates/desktop`: `yandex_backend.rs` (keep every gallery item, implement `download`; `fetch_media` keeps refusing `MediaKind::File` so originals can never take the in-memory path), `store.rs` (download state; gallery thumbnails requested, originals never auto-requested), `timeline.rs` (download button on the file card, gallery grid), a new viewer module and `desktop.rs` (overlay, keys), `main.rs` (Downloads folder in the client config). `README.md`: the sign-out sentence about downloaded media.
- `bt-probe`: a download command (any chat; read-only) for re-checking the original-file URL with the Rust client.
- Risks: the protocol is unofficial and reverse-engineered, URL templates come from runtime config and may change; downloads write to the user's disk and may store private data. The file-download URL template was verified live on 2026-11-24 (Decision 5), but only for images stored on Disk.

## Decisions

Every open question of the first draft was resolved by taking its recommended answer, as instructed for autonomous work. Rationale and trade-offs below; the technical consequences are in `design.md`.

1. **Download trigger: explicit "Download" button.** Chosen over automatic save. It avoids saving private files and wasting disk by accident; the cost is one extra click per file. Downloading is a read-only user action, so the write allowlist and the `message-sending` rule "Writes only from user actions" are unaffected.
2. **Destination and overwrite policy: the user's Downloads folder, never overwrite.** An existing name gets a numeric suffix (`report (1).pdf`); the account cache keeps thumbnails only. Trade-off accepted: saved files live outside the runtime directory and are not removed with the account (sign-out does not delete what the user saved), which is what "download" means to the user.
3. **Gallery presentation: grid inside the message plus a viewer.** Chosen over a sequential list. It keeps every wire `Gallery` item and matches Telegram Desktop's behavior (no code is copied, per project context); the cost is a viewer component and grid layout.
4. **Size and type limits: originals are streamed to disk with no hard cap, with visible progress and Cancel.** The 25 MiB cap in `media.rs` stays for in-memory previews only. Trade-off accepted: a huge file can fill the disk until cancelled; a write failure (disk full) ends the download as failed and removes the partial file. No type restrictions: any file is saved verbatim and never opened or executed by the app.
5. **Live check of the original-file URL: done before the specs were finalized, with the stored session of the signed-in account.** Read-only GET requests (the same cookie rule as `MediaClient`, no redirect forwarding of the cookie) against two images of a group chat (ids of the form `disk/<uuid>`, i.e. `Source` Disk): `https://files.messenger.yandex.ru/file_shortterm/{Id2}?attach=true` answers `302` (cookie sent only to `files.messenger.yandex.ru`) → `302` from `downloader.disk.yandex.ru` → `200` from `mds.storage.yandex.net` with `Content-Length`, `Content-Disposition: attachment` and the right `Content-Type`; no cookie was sent to the later hops and none was needed; the body size equalled `FileInfo.Size` for both images (62962 and 240897 bytes). The self chat holds no files, so the check used a group chat; `bt-probe download` (task 1.2) is read-only and takes any chat. Not covered: a file above 25 MiB (several exist in the same group chat, the largest about 160 MB; task 1.3 downloads one with the Rust client), `MiscFile` non-image files, and ids with a bucket other than `disk`. The spec still states behavior only, so these gaps cannot invalidate its scenarios; the details are in `design.md` "Live verification".

## Follow-ups

1. Upload and send media: image/file selection, Disk upload flow (`upload_to_disk` → HTTP PUT → `add_files`), message payload with the resulting `file_id`; explicit target-chat confirmation; orphaned server file risk when the send fails.
2. Voice/video playback: confirm URLs and formats, GPUI/macOS audio/video support, play on explicit click only; voice upload and waveform only if the chosen UX needs them.
3. After saving: "Show in Finder" and remembering saved files across restarts (needs persistence of the destination path).
