# Proposal

> Single context document for this change. Full research: [`research.md`](research.md) (roadmap item 2, media). Design, specs and tasks are not written yet: the Open Questions below need user decisions first.

## Why

Attachments are visible but inert: an image shows only a middle-size thumbnail, a gallery collapses to its first image, and a file card cannot be opened or saved. To replace the web client for daily use the user must be able to get the file out of a message and look at every image of a gallery. This is the first of three slices of roadmap item 2 (media); it is read-only, so it does not touch the write-request allowlist introduced by `add-sending`.

## What Changes

- Explicit file download: a "Download" action on the file card saves the original to disk. Nothing is downloaded automatically when history loads (only avatars and image thumbnails, as today).
- Gallery becomes a first-class message body: the application model keeps all images (today `yandex_backend.rs` keeps only the first), the timeline shows them as a grid.
- Image viewer: clicking an image or a grid cell opens a viewer for the message's images with previous/next navigation.
- Media layer: `MediaTarget`/`MediaKind` gain an original-file target alongside `Avatar` and `Thumbnail`; the download goes through the existing authenticated `MediaClient` GET path (cookie only for the matching host), with progress, cancel and failure states. Size limit handling for originals is decided in Open Questions (the current 25 MiB cap is for previews).
- Offline and failure behavior: a failed or interrupted download leaves no partial file under the final name and can be retried; offline shows the action as unavailable.

Non-goals: uploading and sending images or files (slice 2); voice and video playback, waveform, video streams (slice 3); `get_media_messages` media browser; automatic download of any original; deleting server-side files; drag-and-drop.

## Capabilities

### New Capabilities
- `attachment-download`: explicit saving of a message's file or image original to disk: action, destination, overwrite policy, progress, cancel, failure and retry.

### Modified Capabilities
- `message-history`: the "Attachments preview" requirement changes: "Opening or downloading files is out of scope" is removed (moved to `attachment-download`), and galleries show all images in a grid with an image viewer instead of a single thumbnail.

## Impact

- `crates/yandex-proto`: `media.rs` (original-file target and URL template, size limit for originals, streaming to a file instead of a memory buffer), `wire.rs` (`Id2` handle for files; gallery already parsed), `client.rs` facade.
- `crates/chat-core`: `MessageBody` gains a gallery variant; `MediaKind::File` gets a real fetch path; download state (progress, failed, done) kept out of the thumbnail cache in `cache.rs`.
- `crates/desktop`: `yandex_backend.rs` (stop dropping gallery items, stop rejecting `MediaKind::File`), `store.rs` (originals never auto-requested), `timeline.rs` (download button, gallery grid, viewer).
- `bt-probe`: a download command for live verification of the original-file URL against the self chat.
- Risks: the protocol is unofficial and reverse-engineered, URL templates come from runtime config and may change; downloads write to the user's disk and may store private data; the file-download URL template is confirmed only by the reference, not yet by a live check.

## Follow-ups

1. Upload and send media: image/file selection, Disk upload flow (`upload_to_disk` → HTTP PUT → `add_files`), message payload with the resulting `file_id`; explicit target-chat confirmation; orphaned server file risk when the send fails.
2. Voice/video playback: confirm URLs and formats, GPUI/macOS audio/video support, play on explicit click only; voice upload and waveform only if the chosen UX needs them.

## Open Questions

1. **Download trigger: explicit "Download" button or automatic save?** Recommended: explicit button. It avoids saving private files and wasting disk by accident; the cost is one extra click per file.
2. **Destination and overwrite policy: the user's Downloads folder or an account cache folder?** Recommended: the user's Downloads folder, never overwrite an existing file (add a numeric suffix), cache stays for thumbnails only. Trade-off: files outside the app's runtime directory are not cleaned up with the account, but that is what the user expects from "download".
3. **Gallery presentation: grid inside the message plus a viewer, or a sequential list only?** Recommended: grid plus viewer. It keeps every wire `Gallery` item and matches Telegram Desktop; costs a new viewer component and layout work versus a simple vertical list.
4. **Size and type limits: what maximum size for originals, and what happens beyond it?** The 25 MiB cap in `media.rs` is for preview GETs and is not confirmed as the right limit for files. Recommended: stream originals to disk with no hard cap and a visible progress/cancel, keeping the 25 MiB cap for in-memory previews only. Trade-off: no cap means a huge file can fill the disk until cancelled; a cap blocks legitimate large files.
5. **Confirm the original-file URL against a live account before specs?** The URL template and `Id2` handle come from a third-party reference and runtime config, not from a live check. Recommended: yes, probe once with `bt-probe` on a file in the self chat before writing specs; trade-off: one extra step, but a wrong template would invalidate the spec's scenarios.
