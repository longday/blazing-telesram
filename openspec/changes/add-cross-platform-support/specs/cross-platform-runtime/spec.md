# Spec Delta

## Purpose

The client runs from source on macOS, Linux and Windows and behaves the same on each: what it stores stays private to the OS user, files are saved where and under names the host accepts, times are in the user's zone, the usual shortcuts work, the window can be placed, moved and closed, voice messages play, and a missing prerequisite is explained. Differences between platforms are listed here and nowhere else.

## ADDED Requirements

### Requirement: Runs on macOS, Linux and Windows
The client SHALL start from source, through the run script of its platform, on macOS, on Linux under Wayland or X11, and on Windows 10 or later (64-bit), and on each SHALL offer the same features: sign-in, the chat list, history, search, reactions, sending, editing, downloads, voice playback, the log view and sign-out. A feature SHALL NOT be offered on one platform and silently missing on another; where a platform lacks the means for one, this specification names the difference.

#### Scenario: First launch on each platform
- **GIVEN** no stored session and the build prerequisites of the platform installed
- **WHEN** the run script of the platform is started
- **THEN** the sign-in window opens and, after a sign-in, the chat window opens with the account's chats

#### Scenario: Stored session on each platform
- **GIVEN** a stored session
- **WHEN** the client starts
- **THEN** the chat window opens without the sign-in window

### Requirement: Local data readable only by the OS user
Everything the client writes under its runtime directory (the stored session, the offline cache, media and playback copies, log files) SHALL be readable and writable only by the current OS user from the moment the file or directory is created, on every platform: owner-only permissions on macOS and Linux, and on Windows an access list that grants only the current user. The client SHALL NOT widen the access of the runtime directory it was pointed at.

#### Scenario: Session file on macOS and Linux
- **WHEN** the client writes the session
- **THEN** the file's mode allows no access to group or others

#### Scenario: Session file on Windows
- **WHEN** the client writes the session
- **THEN** the file's access list has no inherited entries and only the current user can read or change it

#### Scenario: Directories created by the client
- **WHEN** the client creates the cache, media, playback or log directory
- **THEN** that directory is private in the same sense, and so is every file created in it

#### Scenario: Never a wider moment
- **WHEN** the session file, the cache database or a log file is created
- **THEN** it is never, even for an instant, accessible to another user

### Requirement: Downloads folder of the platform
The folder named «Downloads folder» in `attachment-download` SHALL be the user's Downloads folder as the platform defines it: `Downloads` in the home folder on macOS, the Downloads known folder on Windows (including when the user moved it), and the XDG download directory on Linux (including a localised one).

#### Scenario: Windows with a moved Downloads folder
- **GIVEN** the user moved the Downloads folder to another drive
- **WHEN** an attachment is downloaded
- **THEN** it is saved in the moved folder

#### Scenario: Linux with a localised folder
- **GIVEN** the XDG download directory is `~/Загрузки`
- **WHEN** an attachment is downloaded
- **THEN** it is saved there and not in `~/Downloads`

### Requirement: File names valid on the host
A saved file's name SHALL be valid on the host file system as well as safe by the rules of `attachment-download`: on Windows the characters `<`, `>`, `:`, `"`, `|`, `?`, `*` and control characters SHALL NOT survive, a name SHALL NOT end with a dot or a space, and a reserved device name SHALL be changed so that the result is an ordinary file. Saving SHALL never overwrite an existing file, also on a file system that cannot make hard links.

#### Scenario: Forbidden characters on Windows
- **WHEN** an attachment named `a:b*c?.txt` is downloaded on Windows
- **THEN** a single ordinary file with the same visible name except for the forbidden characters is saved in the Downloads folder

#### Scenario: Reserved name
- **WHEN** an attachment named `CON.txt` is downloaded on Windows
- **THEN** a file that can be opened as an ordinary file is saved, and no device is written

#### Scenario: Trailing dot
- **WHEN** an attachment named `report.` is downloaded on Windows
- **THEN** the saved name does not end with a dot

#### Scenario: Unchanged on macOS
- **WHEN** the same names are downloaded on macOS
- **THEN** the saved names are those the rules of `attachment-download` gave before this requirement

#### Scenario: No hard links
- **GIVEN** the Downloads folder is on a file system without hard links and already holds `report.pdf`
- **WHEN** another attachment named `report.pdf` is downloaded
- **THEN** it is saved as `report (1).pdf` and the existing file is unchanged

### Requirement: Times in the user's zone
Message times and dates SHALL be shown in the zone the operating system is set to, including its daylight saving, on every platform.

#### Scenario: Zone on Windows
- **GIVEN** Windows is set to a zone with an offset of +05:30
- **WHEN** a message sent at 12:00 UTC is shown
- **THEN** it shows 17:30

### Requirement: Shortcuts follow the platform
The shortcuts of the client (search, the log view, quit, copy, cut, paste, select all, undo in the text input, and any shortcut a later change adds) SHALL use the platform's primary modifier: Command on macOS and Control on Linux and Windows. The Windows or Super key SHALL NOT be a shortcut modifier. Quit SHALL also be reachable by closing the last window on every platform.

#### Scenario: Search on Linux and Windows
- **WHEN** the user presses Control+F in the chat window
- **THEN** the search field takes the focus

#### Scenario: Search on macOS
- **WHEN** the user presses Command+F in the chat window
- **THEN** the search field takes the focus

#### Scenario: Copy in the text input
- **GIVEN** the composer holds selected text
- **WHEN** the user presses the primary modifier with C
- **THEN** the selection is on the clipboard

#### Scenario: Super key
- **WHEN** the user presses the Windows or Super key with F
- **THEN** the client does nothing

### Requirement: Window placement and management
The chat window and the sign-in window SHALL open fully on the display, at a size that fits it with room for the platform's task bar and title bar. The user SHALL always be able to move, resize, minimise (where the platform has it), maximise and close the window: when the desktop draws the window's title bar the client SHALL NOT draw another; when it does not (a Wayland compositor without server-side decorations), the client SHALL draw a title bar and resize edges with the same controls.

#### Scenario: Small display
- **GIVEN** a display smaller than the window's preferred size
- **WHEN** the chat window opens
- **THEN** the window fits inside the display

#### Scenario: Desktop without decorations
- **GIVEN** a Linux session whose compositor does not draw window decorations
- **WHEN** the chat window opens
- **THEN** it shows a title bar with the window title and the minimise, maximise and close controls, can be moved by dragging the title bar, and can be resized from its edges

#### Scenario: Desktop with decorations
- **GIVEN** a platform that draws window decorations
- **WHEN** the chat window opens
- **THEN** the client draws no title bar of its own

### Requirement: Voice messages play on every platform
A voice message SHALL play with the behaviour of `media-playback` on macOS, Linux and Windows: the same controls, the exact length of the recording, a position that advances only while sound plays, pause and resume, one at a time, and a stop at the end. When the computer has no sound output the client SHALL report that as it does today and SHALL NOT fail otherwise.

#### Scenario: Play a voice message on Linux and Windows
- **GIVEN** a voice message in a chat
- **WHEN** the user presses play
- **THEN** the sound plays from the default output device and the progress follows it

#### Scenario: Length
- **WHEN** a voice message's recording is opened for playback
- **THEN** the shown length is the recording's own length, not the length the server reported

#### Scenario: No sound output
- **GIVEN** the computer has no sound output device
- **WHEN** the user presses play
- **THEN** the voice message shows the existing «no sound output» outcome and the rest of the client is unaffected

### Requirement: A missing prerequisite is explained
When something the client needs from the system is missing at start (the web view runtime for the sign-in, the graphics driver for the window, the system's file opener), the client SHALL say what is missing and what to install, in one message, instead of failing silently or with a bare error code, and SHALL NOT leave a half-written session or cache behind.

#### Scenario: No web view runtime
- **GIVEN** the platform's web view runtime is not installed
- **WHEN** the sign-in is needed
- **THEN** a message names the runtime to install and the client quits when it is closed, with nothing stored

#### Scenario: No graphics driver on Linux
- **GIVEN** no Vulkan driver is installed
- **WHEN** the client starts
- **THEN** the terminal shows one line naming the package to install

### Requirement: macOS behaviour is unchanged
Adding support for Linux and Windows SHALL NOT change what the client does on macOS: its shortcuts, labels, window, sign-in view, audio, Dock badge and storage behave as before.

#### Scenario: Existing behaviour
- **WHEN** the client runs on macOS after this change
- **THEN** every behaviour specified before this change holds unchanged
